//! Cursor, two stores:
//!
//! - the CLI (`cursor-agent`): `~/.cursor/chats/<md5(cwd)>/<session>/` with
//!   `meta.json` and `store.db` (SQLite: `meta` row `0` = hex-encoded JSON
//!   with `latestRootBlobId`; `blobs` = content-addressed messages in AI-SDK
//!   shape, linked by protobuf tree blobs). Exported with a JSON dump of the
//!   store, so it is redacted like any text and restored as a new
//!   `store.db` (`cursor-agent --resume <id>`).
//! - the editor: `<config>/Cursor/User/globalStorage/state.vscdb`
//!   (`cursorDiskKV`: `composerData:<id>`, `bubbleId:<id>:<bubble>`), matched
//!   to the folder through `workspaceStorage/*/workspace.json`. Readable
//!   only: writing into the editor's database while it runs is not safe.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use md5::{Digest, Md5};
use rusqlite::{types::ValueRef, Connection, OpenFlags, OptionalExtension};
use serde_json::{json, Value};

use super::{home, same_dir, Converted, Meta, NativeFile, SessionRef, Tool};
use crate::atif::{Builder, Usage};

pub const CLI_LAYOUT: &str = "cursor/chats-v1";
pub const DUMP_FORMAT: &str = "flightlog.cursor-store/1";

pub fn cursor_home() -> PathBuf {
    std::env::var_os("CURSOR_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".cursor"))
}

/// The editor's `User` folder (`CURSOR_USER_DIR` overrides it).
fn ide_user_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("CURSOR_USER_DIR") {
        return Some(PathBuf::from(d));
    }
    Some(dirs::config_dir()?.join("Cursor").join("User"))
}

/// The CLI's folder name for a working directory.
pub fn cwd_hash(cwd: &str) -> String {
    hex(&Md5::digest(cwd.as_bytes()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

fn open_ro(path: &Path) -> Result<Connection> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("open {}", path.display()))
}

fn bytes_of(v: ValueRef) -> Option<Vec<u8>> {
    match v {
        ValueRef::Text(t) | ValueRef::Blob(t) => Some(t.to_vec()),
        _ => None,
    }
}

fn get_json(conn: &Connection, table: &str, key: &str) -> Option<Value> {
    let sql = format!("SELECT value FROM {table} WHERE key = ?1");
    let raw: Option<Vec<u8>> = conn
        .query_row(&sql, [key], |r| Ok(bytes_of(r.get_ref(0)?)))
        .optional()
        .ok()??;
    serde_json::from_slice(&raw?).ok()
}

fn ms_to_rfc3339(ms: i64) -> Option<String> {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}

fn time_of(v: &Value) -> Option<String> {
    match v {
        Value::Number(n) => n.as_i64().and_then(ms_to_rfc3339),
        Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

/// Arguments recorded as a JSON string or as an object.
fn args_of(v: &Value) -> Value {
    match v {
        Value::String(s) => serde_json::from_str(s).unwrap_or_else(|_| json!({ "input": s })),
        Value::Null => json!({}),
        v => v.clone(),
    }
}

fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Array(a) => a.iter().map(text_of).collect::<Vec<_>>().join("\n"),
        Value::Object(o) => o
            .get("text")
            .or(o.get("value"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| v.to_string()),
        v => v.to_string(),
    }
}

// ─── CLI sessions ────────────────────────────────────────────────────────────

fn cli_sessions() -> Vec<(PathBuf, Value)> {
    let mut out = Vec::new();
    let Ok(projects) = std::fs::read_dir(cursor_home().join("chats")) else {
        return out;
    };
    for p in projects.flatten() {
        let Ok(sessions) = std::fs::read_dir(p.path()) else {
            continue;
        };
        for s in sessions.flatten() {
            let dir = s.path();
            if !dir.join("store.db").is_file() {
                continue;
            }
            let meta = crate::safe_fs::read_text(&dir.join("meta.json"), 1024 * 1024)
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or(Value::Null);
            out.push((dir, meta));
        }
    }
    out
}

fn cli_ref(dir: &Path, meta: &Value) -> SessionRef {
    SessionRef {
        tool: Tool::Cursor,
        id: dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        modified: meta["updatedAtMs"]
            .as_u64()
            .map(|ms| std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms))
            .or_else(|| super::modified(&dir.join("store.db"))),
        title: meta["title"].as_str().map(str::to_string),
        path: Some(dir.join("store.db")),
    }
}

/// Every blob id reachable from `id`, messages in conversation order.
fn walk_blobs(
    conn: &Connection,
    id: &str,
    seen: &mut HashSet<String>,
    out: &mut Vec<Value>,
) -> Result<()> {
    let mut pending = vec![id.to_string()];
    let mut total = 0usize;
    while let Some(id) = pending.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        if seen.len() > 10_000 {
            bail!("Cursor graph exceeds 10000 blobs");
        }
        let data: Vec<u8> = conn
            .query_row(
                "SELECT data FROM blobs WHERE id = ?1 AND length(data) <= 10485760",
                [&id],
                |r| Ok(bytes_of(r.get_ref(0)?).unwrap_or_default()),
            )
            .context("missing or oversized Cursor blob")?;
        total = total.saturating_add(data.len());
        if total > crate::safe_fs::MAX_FILE_BYTES {
            bail!("Cursor graph exceeds size limit");
        }
        if let Ok(v @ Value::Object(_)) = serde_json::from_slice::<Value>(&data) {
            out.push(v);
            continue;
        }
        // Only the supported tree format: repeated field 1, 32-byte blob IDs.
        if data.is_empty()
            || !data.len().is_multiple_of(34)
            || data.chunks_exact(34).any(|c| c[..2] != [0x0a, 0x20])
        {
            bail!("unsupported Cursor tree format");
        }
        for chunk in data.chunks_exact(34).rev() {
            pending.push(hex(&chunk[2..]));
        }
        if pending.len() > 10_000 {
            bail!("Cursor graph exceeds size limit");
        }
    }
    Ok(())
}

fn store_meta(conn: &Connection) -> Result<Value> {
    let raw: String = conn
        .query_row("SELECT value FROM meta WHERE key = '0'", [], |r| r.get(0))
        .context("Cursor store without meta")?;
    let bytes = unhex(&raw).unwrap_or_else(|| raw.clone().into_bytes());
    serde_json::from_slice(&bytes).context("parse Cursor store meta")
}

/// Messages in Vercel AI-SDK shape (`role`, `content` string or blocks).
pub fn convert_messages(messages: &[Value], b: &mut Builder) {
    for m in messages {
        let content = &m["content"];
        let blocks: Vec<Value> = match content {
            Value::Array(a) => a.clone(),
            Value::String(s) => vec![json!({ "type": "text", "text": s })],
            _ => Vec::new(),
        };
        match m["role"].as_str().unwrap_or("") {
            "user" => {
                let text: Vec<&str> = blocks
                    .iter()
                    .filter(|x| x["type"] == "text")
                    .filter_map(|x| x["text"].as_str())
                    .collect();
                let text = text.join("\n");
                if !text.trim().is_empty() {
                    b.add("user", text, None);
                }
            }
            "assistant" => {
                let i = b.add("agent", "", None);
                for x in &blocks {
                    match x["type"].as_str().unwrap_or("") {
                        "text" => b.append_text(i, "message", x["text"].as_str().unwrap_or("")),
                        "reasoning" | "thinking" => b.append_text(
                            i,
                            "reasoning_content",
                            x["text"].as_str().or(x["thinking"].as_str()).unwrap_or(""),
                        ),
                        "tool-call" | "tool_use" => {
                            let args = if x["args"].is_null() {
                                &x["input"]
                            } else {
                                &x["args"]
                            };
                            b.call(
                                i,
                                x["toolCallId"].as_str().or(x["id"].as_str()).unwrap_or(""),
                                x["toolName"]
                                    .as_str()
                                    .or(x["name"].as_str())
                                    .unwrap_or("tool"),
                                args_of(args),
                            );
                        }
                        _ => {}
                    }
                }
                for c in m["tool_calls"].as_array().into_iter().flatten() {
                    b.call(
                        i,
                        c["id"].as_str().unwrap_or(""),
                        c["function"]["name"].as_str().unwrap_or("tool"),
                        args_of(&c["function"]["arguments"]),
                    );
                }
            }
            "tool" => {
                for x in &blocks {
                    if x["type"] == "tool-result" || x["type"] == "tool_result" {
                        let out = if x["result"].is_null() {
                            text_of(if x["output"].is_null() {
                                &x["content"]
                            } else {
                                &x["output"]
                            })
                        } else {
                            text_of(&x["result"])
                        };
                        b.observe(
                            x["toolCallId"]
                                .as_str()
                                .or(x["tool_use_id"].as_str())
                                .unwrap_or(""),
                            &out,
                        );
                    }
                }
                if let Some(id) = m["tool_call_id"].as_str() {
                    b.observe(id, &text_of(content));
                }
            }
            _ => {}
        }
    }
}

/// The whole store as JSON: JSON blobs as values (so redaction reaches
/// them), others as hex.
fn dump_store(conn: &Connection, meta_json: &Value, reachable: &HashSet<String>) -> Result<Value> {
    let schema = [
        "CREATE TABLE blobs (id TEXT PRIMARY KEY, data BLOB)",
        "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT)",
    ];
    let mut blobs = Vec::new();
    let mut ids: Vec<_> = reachable.iter().collect();
    ids.sort();
    for id in ids {
        let data: Vec<u8> = conn.query_row(
            "SELECT data FROM blobs WHERE id=?1 AND length(data)<=10485760",
            [id],
            |r| Ok(bytes_of(r.get_ref(0)?).unwrap_or_default()),
        )?;
        match serde_json::from_slice::<Value>(&data) {
            Ok(v) if data.first() == Some(&b'{') => blobs.push(json!({ "id": id, "json": v })),
            _ => blobs.push(json!({ "id": id, "hex": hex(&data) })),
        }
    }
    Ok(json!({
        "format": DUMP_FORMAT,
        "schema": schema,
        "meta_json": meta_json,
        "store_meta": store_meta(conn)?,
        "blobs": blobs,
    }))
}

fn convert_cli(s: &SessionRef, store: &Path) -> Result<Converted> {
    let dir = store.parent().context("store without folder")?;
    let meta_json: Value = crate::safe_fs::read_text(&dir.join("meta.json"), 1024 * 1024)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Value::Null);
    let conn = open_ro(store)?;
    let sm = store_meta(&conn)?;
    let root = sm["latestRootBlobId"]
        .as_str()
        .context("Cursor store without latestRootBlobId")?;
    let mut messages = Vec::new();
    let mut reachable = HashSet::new();
    walk_blobs(&conn, root, &mut reachable, &mut messages)?;
    let mut b = Builder::default();
    convert_messages(&messages, &mut b);
    let mut meta = Meta {
        cwd: meta_json["cwd"].as_str().map(str::to_string),
        title: meta_json["title"]
            .as_str()
            .or(sm["name"].as_str())
            .map(str::to_string),
        started: meta_json["createdAtMs"]
            .as_i64()
            .or(sm["createdAt"].as_i64())
            .and_then(ms_to_rfc3339),
        ended: meta_json["updatedAtMs"].as_i64().and_then(ms_to_rfc3339),
        ..Default::default()
    };
    if let Some(m) = sm["lastUsedModel"].as_str() {
        meta.models.insert(m.to_string());
    }
    let dump = dump_store(&conn, &meta_json, &reachable)?;
    let totals = b.totals();
    Ok(Converted {
        tool: Tool::Cursor,
        session_id: s.id.clone(),
        agent_name: "cursor-agent",
        steps: b.finish(),
        totals,
        meta,
        native: vec![NativeFile {
            path: format!("native/{}.cursor-store.json", s.id),
            bytes: serde_json::to_vec(&dump)?,
            restore_to: None,
        }],
        layout: CLI_LAYOUT,
        resume: format!("cursor-agent --resume {}", s.id),
    })
}

/// Only understood JSON messages and trees of blob references may cross the boundary.
pub fn validate_dump(d: &Value) -> Result<()> {
    if d["format"] != DUMP_FORMAT {
        bail!("invalid Cursor dump");
    }
    let blobs = d["blobs"].as_array().context("missing Cursor blobs")?;
    if blobs.len() > 10_000 {
        bail!("too many Cursor blobs");
    }
    let ids: HashSet<_> = blobs.iter().filter_map(|b| b["id"].as_str()).collect();
    if ids.len() != blobs.len()
        || ids
            .iter()
            .any(|id| id.len() != 64 || !id.bytes().all(|c| c.is_ascii_hexdigit()))
    {
        bail!("invalid Cursor blob IDs");
    }
    let root = d["store_meta"]["latestRootBlobId"]
        .as_str()
        .context("missing Cursor root")?;
    if !ids.contains(root) {
        bail!("missing Cursor root blob");
    }
    for b in blobs {
        if let Some(h) = b["hex"].as_str() {
            let raw = unhex(h).context("invalid Cursor tree")?;
            if !b["json"].is_null()
                || raw.is_empty()
                || !raw.len().is_multiple_of(34)
                || raw
                    .chunks_exact(34)
                    .any(|c| c[0] != 0x0a || c[1] != 0x20 || !ids.contains(hex(&c[2..]).as_str()))
            {
                bail!("unsupported opaque Cursor content; use --no-native");
            }
        } else if !b["json"].is_object() {
            bail!("unsupported Cursor blob; use --no-native");
        }
    }
    Ok(())
}

/// Rebuild a CLI session from its dump under `cwd`'s folder; returns the
/// session folder.
pub fn restore_cli(dump: &[u8], session_id: &str, cwd: &str, force: bool) -> Result<PathBuf> {
    let d: Value = serde_json::from_slice(dump).context("parse Cursor store dump")?;
    validate_dump(&d)?;
    if d["format"] != DUMP_FORMAT {
        bail!("unknown Cursor store dump format");
    }
    if !crate::safe_fs::identifier(session_id) {
        bail!("invalid session id");
    }
    let dir = cursor_home()
        .join("chats")
        .join(cwd_hash(cwd))
        .join(session_id);
    crate::safe_fs::check_destination(&dir.join("store.db"), force)?;
    crate::safe_fs::check_destination(&dir.join("meta.json"), force)?;
    // Never execute SQL carried by a bundle, including CREATE TABLE AS SELECT.
    const SCHEMA: &[&str] = &[
        "CREATE TABLE blobs (id TEXT PRIMARY KEY, data BLOB)",
        "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT)",
    ];
    for statement in d["schema"].as_array().into_iter().flatten() {
        let sql = statement.as_str().context("invalid schema")?;
        if !SCHEMA.contains(&sql.trim().trim_end_matches(';')) {
            bail!("unsupported Cursor schema; executable schema is forbidden");
        }
    }
    let staging = tempfile::tempdir()?;
    let database = staging.path().join("store.db");
    let mut conn = Connection::open(&database)?;
    conn.set_db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
    let tx = conn.transaction()?;
    tx.execute_batch("CREATE TABLE blobs (id TEXT PRIMARY KEY, data BLOB); CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);")?;
    if d["blobs"].as_array().is_none_or(|b| b.len() > 10_000) {
        bail!("invalid or excessive Cursor blobs");
    }
    for blob in d["blobs"].as_array().into_iter().flatten() {
        let id = blob["id"].as_str().context("blob without id")?;
        let data = match (&blob["json"], blob["hex"].as_str()) {
            (Value::Null, Some(h)) => unhex(h).context("bad blob hex")?,
            (v, _) => serde_json::to_vec(v)?,
        };
        tx.execute("INSERT INTO blobs (id, data) VALUES (?1, ?2)", (id, data))?;
    }
    let sm = serde_json::to_string(&d["store_meta"])?;
    tx.execute(
        "INSERT INTO meta (key, value) VALUES ('0', ?1)",
        [hex(sm.as_bytes())],
    )?;
    tx.commit()?;
    let mut mj = if d["meta_json"].is_object() {
        d["meta_json"].clone()
    } else {
        json!({ "schemaVersion": 1, "hasConversation": true })
    };
    mj["cwd"] = json!(cwd);
    drop(conn);
    let data = crate::safe_fs::read(&database, crate::safe_fs::MAX_FILE_BYTES)?;
    crate::safe_fs::write(&dir.join("store.db"), &data, force)?;
    crate::safe_fs::write(
        &dir.join("meta.json"),
        &serde_json::to_vec_pretty(&mj)?,
        force,
    )?;
    Ok(dir)
}

// ─── Editor sessions ─────────────────────────────────────────────────────────

/// `file:///c%3A/work/p` → `c:/work/p`; `file:///home/me/p` → `/home/me/p`.
pub fn file_uri_path(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    let mut bytes = Vec::with_capacity(rest.len());
    let raw = rest.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%' && i + 2 < raw.len() {
            if let Ok(b) = u8::from_str_radix(std::str::from_utf8(&raw[i + 1..i + 3]).ok()?, 16) {
                bytes.push(b);
                i += 3;
                continue;
            }
        }
        bytes.push(raw[i]);
        i += 1;
    }
    let p = String::from_utf8(bytes).ok()?;
    // `/c:/work` → `c:/work`
    let b = p.as_bytes();
    if b.len() >= 3 && b[0] == b'/' && b[2] == b':' {
        return Some(p[1..].to_string());
    }
    Some(p)
}

fn collect_composers(v: &Value, out: &mut Vec<Value>) {
    match v {
        Value::Object(o) => {
            if o.contains_key("composerId") {
                out.push(v.clone());
            } else {
                o.values().for_each(|x| collect_composers(x, out));
            }
        }
        Value::Array(a) => a.iter().for_each(|x| collect_composers(x, out)),
        _ => {}
    }
}

fn mentions_dir(v: &Value, ws_ids: &[String], cwd: &str) -> bool {
    match v {
        Value::String(s) => {
            ws_ids.iter().any(|id| id == s)
                || file_uri_path(s).is_some_and(|p| same_dir(&p, cwd))
                || (s.len() > 1 && same_dir(s, cwd))
        }
        Value::Object(o) => o.values().any(|x| mentions_dir(x, ws_ids, cwd)),
        Value::Array(a) => a.iter().any(|x| mentions_dir(x, ws_ids, cwd)),
        _ => false,
    }
}

fn ide_sessions(cwd: &str) -> Vec<SessionRef> {
    let Some(user) = ide_user_dir() else {
        return Vec::new();
    };
    let global = user.join("globalStorage").join("state.vscdb");
    if !global.is_file() {
        return Vec::new();
    }
    let mut ws_ids = Vec::new();
    let mut composers = Vec::new();
    if let Ok(rd) = std::fs::read_dir(user.join("workspaceStorage")) {
        for e in rd.flatten() {
            let ws = e.path();
            let folder = crate::safe_fs::read_text(&ws.join("workspace.json"), 1024 * 1024)
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                .and_then(|v| v["folder"].as_str().and_then(file_uri_path));
            if !folder.is_some_and(|f| same_dir(&f, cwd)) {
                continue;
            }
            ws_ids.push(e.file_name().to_string_lossy().into_owned());
            if let Ok(conn) = open_ro(&ws.join("state.vscdb")) {
                if let Some(v) = get_json(&conn, "ItemTable", "composer.composerData") {
                    collect_composers(&v, &mut composers);
                }
            }
        }
    }
    if let Ok(conn) = open_ro(&global) {
        if let Some(v) = get_json(&conn, "ItemTable", "composer.composerHeaders") {
            let mut all = Vec::new();
            collect_composers(&v, &mut all);
            composers.extend(
                all.into_iter()
                    .filter(|c| mentions_dir(&c["workspaceIdentifier"], &ws_ids, cwd)),
            );
        }
    }
    let mut seen = HashSet::new();
    composers
        .into_iter()
        .filter_map(|c| {
            let id = c["composerId"].as_str()?.to_string();
            seen.insert(id.clone()).then(|| SessionRef {
                tool: Tool::Cursor,
                modified: c["lastUpdatedAt"]
                    .as_u64()
                    .or(c["createdAt"].as_u64())
                    .map(|ms| std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms)),
                title: c["name"].as_str().map(str::to_string),
                path: Some(global.clone()),
                id,
            })
        })
        .collect()
}

pub fn convert_ide_bubbles(bubbles: &[Value], b: &mut Builder, meta: &mut Meta) {
    for x in bubbles {
        let ts = time_of(&x["createdAt"]).or_else(|| {
            x["timingInfo"]["clientStartTime"]
                .as_i64()
                .and_then(ms_to_rfc3339)
        });
        meta.seen(ts.as_deref());
        let text = x["text"].as_str().unwrap_or("");
        match x["type"].as_i64() {
            Some(1) => {
                if !text.trim().is_empty() {
                    b.add("user", text, ts.as_deref());
                }
            }
            Some(2) => {
                let i = b.add("agent", text, ts.as_deref());
                if let Some(m) = x["modelInfo"]["modelName"].as_str() {
                    meta.models.insert(m.to_string());
                    b.set(i, "model_name", json!(m));
                }
                b.append_text(
                    i,
                    "reasoning_content",
                    x["thinking"]["text"].as_str().unwrap_or(""),
                );
                let t = &x["toolFormerData"];
                if t.is_object() && t["name"].is_string() {
                    let id = t["toolCallId"]
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| {
                            format!("{}-tool", x["bubbleId"].as_str().unwrap_or(""))
                        });
                    let args = if t["params"].is_null() {
                        &t["rawArgs"]
                    } else {
                        &t["params"]
                    };
                    b.call(i, &id, t["name"].as_str().unwrap_or("tool"), args_of(args));
                    if !t["result"].is_null() {
                        b.observe(&id, &text_of(&t["result"]));
                    }
                }
                let tc = &x["tokenCount"];
                let u = Usage {
                    prompt: tc["inputTokens"].as_u64().filter(|v| *v > 0),
                    completion: tc["outputTokens"].as_u64().filter(|v| *v > 0),
                    cached: None,
                };
                if let Some(v) = u.to_value() {
                    b.set(i, "metrics", v);
                }
            }
            _ => {}
        }
    }
}

fn convert_ide(s: &SessionRef, global: &Path, cwd: &str) -> Result<Converted> {
    let conn = open_ro(global)?;
    let composer = get_json(&conn, "cursorDiskKV", &format!("composerData:{}", s.id))
        .context("Cursor conversation not found in the editor's database")?;
    let bubbles: Vec<Value> = match composer["fullConversationHeadersOnly"].as_array() {
        Some(headers) => headers
            .iter()
            .filter_map(|h| h["bubbleId"].as_str())
            .filter_map(|bid| get_json(&conn, "cursorDiskKV", &format!("bubbleId:{}:{bid}", s.id)))
            .collect(),
        None => composer["conversation"]
            .as_array()
            .cloned()
            .unwrap_or_default(),
    };
    let mut b = Builder::default();
    let mut meta = Meta {
        cwd: Some(cwd.to_string()),
        title: composer["name"].as_str().map(str::to_string),
        ..Default::default()
    };
    if let Some(m) = composer["modelConfig"]["modelName"].as_str() {
        meta.models.insert(m.to_string());
    }
    convert_ide_bubbles(&bubbles, &mut b, &mut meta);
    if meta.started.is_none() {
        meta.started = time_of(&composer["createdAt"]);
        meta.ended = time_of(&composer["lastUpdatedAt"]);
    }
    let totals = b.totals();
    Ok(Converted {
        tool: Tool::Cursor,
        session_id: s.id.clone(),
        agent_name: "cursor",
        steps: b.finish(),
        totals,
        meta,
        native: Vec::new(),
        layout: "cursor/editor-v1",
        resume: String::new(),
    })
}

// ─── Entry points ────────────────────────────────────────────────────────────

pub fn list(cwd: &str) -> Result<Vec<SessionRef>> {
    let hash = cwd_hash(cwd);
    let mut v: Vec<SessionRef> = cli_sessions()
        .into_iter()
        .filter(|(dir, meta)| {
            meta["cwd"].as_str().is_some_and(|c| same_dir(c, cwd))
                || dir
                    .parent()
                    .and_then(Path::file_name)
                    .is_some_and(|n| n.to_string_lossy() == hash)
        })
        .map(|(dir, meta)| cli_ref(&dir, &meta))
        .collect();
    v.extend(ide_sessions(cwd));
    v.sort_by_key(|s| std::cmp::Reverse(s.modified));
    Ok(v)
}

pub fn by_id(id: &str, cwd: &str) -> Option<SessionRef> {
    cli_sessions()
        .into_iter()
        .find(|(dir, _)| dir.file_name().is_some_and(|n| n.to_string_lossy() == id))
        .map(|(dir, meta)| cli_ref(&dir, &meta))
        .or_else(|| ide_sessions(cwd).into_iter().find(|s| s.id == id))
}

pub fn convert(s: &SessionRef, cwd: &str) -> Result<Converted> {
    let path = s.path.as_ref().context("Cursor session without a store")?;
    if path.file_name().is_some_and(|n| n == "store.db") {
        convert_cli(s, path)
    } else {
        convert_ide(s, path, cwd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_store(dir: &Path) -> String {
        let conn = Connection::open(dir.join("store.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE blobs (id TEXT PRIMARY KEY, data BLOB);
             CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);",
        )
        .unwrap();
        let msgs = [
            json!({"role": "system", "content": "you are cursor"}),
            json!({"role": "user", "content": [{"type": "text", "text": "fix the bug"}]}),
            json!({"role": "assistant", "content": [
                {"type": "reasoning", "text": "look at main.rs"},
                {"type": "text", "text": "Reading it."},
                {"type": "tool-call", "toolCallId": "t1", "toolName": "read_file", "args": {"path": "main.rs"}}]}),
            json!({"role": "tool", "content": [
                {"type": "tool-result", "toolCallId": "t1", "toolName": "read_file", "result": "fn main() {}"}]}),
            json!({"role": "assistant", "content": "Done."}),
        ];
        let mut tree = Vec::new();
        for m in &msgs {
            let data = serde_json::to_vec(m).unwrap();
            let id = sha2::Sha256::digest(&data);
            conn.execute("INSERT INTO blobs VALUES (?1, ?2)", (hex(&id), data))
                .unwrap();
            tree.extend([0x0a, 0x20]);
            tree.extend_from_slice(&id);
        }
        use sha2::Digest as _;
        let root = hex(&sha2::Sha256::digest(&tree));
        conn.execute("INSERT INTO blobs VALUES (?1, ?2)", (&root, tree))
            .unwrap();
        let meta = json!({"agentId": "a", "latestRootBlobId": root, "name": "Fix bug", "lastUsedModel": "gpt-5"});
        conn.execute(
            "INSERT INTO meta VALUES ('0', ?1)",
            [hex(meta.to_string().as_bytes())],
        )
        .unwrap();
        root
    }

    #[test]
    fn cli_store_walks_tree_and_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("chats").join("h").join("sess-1");
        std::fs::create_dir_all(&dir).unwrap();
        fake_store(&dir);
        std::fs::write(
            dir.join("meta.json"),
            r#"{"schemaVersion":1,"createdAtMs":1758362400000,"updatedAtMs":1758362500000,"hasConversation":true,"cwd":"/work/p"}"#,
        )
        .unwrap();
        let s = SessionRef {
            tool: Tool::Cursor,
            id: "sess-1".into(),
            path: Some(dir.join("store.db")),
            modified: None,
            title: None,
        };
        let c = convert(&s, "/work/p").unwrap();
        assert_eq!(c.steps.len(), 3, "{:#?}", c.steps);
        assert_eq!(c.steps[0]["message"], "fix the bug");
        assert_eq!(c.steps[1]["reasoning_content"], "look at main.rs");
        assert_eq!(c.steps[1]["tool_calls"][0]["function_name"], "read_file");
        assert_eq!(
            c.steps[1]["observation"]["results"][0]["content"],
            "fn main() {}"
        );
        assert_eq!(c.steps[2]["message"], "Done.");
        assert!(c.meta.models.contains("gpt-5"));

        // Restore the dump elsewhere and read it back.
        std::env::set_var("CURSOR_CONFIG_DIR", tmp.path().join("other"));
        let out = restore_cli(&c.native[0].bytes, "sess-1", "/work/q", false).unwrap();
        assert!(out.ends_with(Path::new(&cwd_hash("/work/q")).join("sess-1")));
        let s2 = SessionRef {
            path: Some(out.join("store.db")),
            ..s
        };
        let c2 = convert(&s2, "/work/q").unwrap();
        assert_eq!(c2.steps, c.steps);
        assert!(restore_cli(&c.native[0].bytes, "sess-1", "/work/q", false).is_err());
        std::env::remove_var("CURSOR_CONFIG_DIR");
    }

    #[test]
    fn editor_bubbles() {
        let bubbles = vec![
            json!({"type": 1, "bubbleId": "b1", "text": "hi", "createdAt": 1758362400000i64}),
            json!({"type": 2, "bubbleId": "b2", "text": "hello", "thinking": {"text": "greet"},
                   "modelInfo": {"modelName": "claude-4"},
                   "toolFormerData": {"name": "list_dir", "params": "{\"path\":\".\"}", "result": "a b"},
                   "tokenCount": {"inputTokens": 10, "outputTokens": 3}}),
        ];
        let mut b = Builder::default();
        let mut meta = Meta::default();
        convert_ide_bubbles(&bubbles, &mut b, &mut meta);
        let steps = b.finish();
        assert_eq!(steps[1]["tool_calls"][0]["arguments"]["path"], ".");
        assert_eq!(steps[1]["observation"]["results"][0]["content"], "a b");
        assert_eq!(steps[1]["metrics"]["completion_tokens"], 3);
        assert!(meta.models.contains("claude-4"));
    }

    #[test]
    fn file_uris() {
        assert_eq!(
            file_uri_path("file:///home/me/p").as_deref(),
            Some("/home/me/p")
        );
        assert_eq!(
            file_uri_path("file:///c%3A/work/My%20P").as_deref(),
            Some("c:/work/My P")
        );
        assert!(same_dir(
            &file_uri_path("file:///C%3A/work/p").unwrap(),
            r"C:\work\p"
        ));
    }
}
