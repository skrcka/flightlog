//! Gemini CLI: `~/.gemini/tmp/<project>/chats/session-<ts>-<id8>.jsonl`.
//!
//! `<project>` is a slug of the project folder's name, recorded in
//! `~/.gemini/projects.json` (older versions used `sha256(project root)`).
//! A session file is JSONL: a metadata line, then message records (a
//! repeated `id` replaces the earlier one), `{"$set": …}` metadata updates
//! (`$set.messages` replaces them all) and `{"$rewindTo": id}` truncations.
//! Older versions wrote one JSON document with a `messages` array.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::{home, modified, plain_path, same_dir, Converted, Meta, NativeFile, SessionRef, Tool};
use crate::atif::{Builder, Usage};

pub fn gemini_home() -> PathBuf {
    std::env::var_os("GEMINI_CLI_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(home)
        .join(".gemini")
}

fn tmp_dir() -> PathBuf {
    gemini_home().join("tmp")
}

fn registry() -> Value {
    std::fs::read_to_string(gemini_home().join("projects.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({ "projects": {} }))
}

/// sha256 of the project root, Gemini's `projectHash`.
pub fn project_hash(cwd: &str) -> String {
    let mut h = Sha256::new();
    h.update(cwd.as_bytes());
    format!("{:x}", h.finalize())
}

/// Gemini's slug rule: the folder name, lowercased, runs of anything outside
/// `a-z0-9` as one `-`, no leading or trailing `-`.
pub fn slug(cwd: &str) -> String {
    let name = plain_path(cwd)
        .replace('\\', "/")
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_lowercase();
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "project".into()
    } else {
        out
    }
}

/// Folders under `~/.gemini/tmp` that may hold `cwd`'s sessions.
fn project_dirs(cwd: &str) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(map) = registry()["projects"].as_object() {
        for (root, s) in map {
            if same_dir(root, cwd) {
                if let Some(s) = s.as_str() {
                    dirs.push(tmp_dir().join(s));
                }
            }
        }
    }
    if let Ok(rd) = std::fs::read_dir(tmp_dir()) {
        for e in rd.flatten() {
            let p = e.path();
            let owner = std::fs::read_to_string(p.join(".project_root")).unwrap_or_default();
            if !owner.trim().is_empty() && same_dir(owner.trim(), cwd) && !dirs.contains(&p) {
                dirs.push(p);
            }
        }
    }
    let legacy = tmp_dir().join(project_hash(cwd));
    if legacy.is_dir() && !dirs.contains(&legacy) {
        dirs.push(legacy);
    }
    dirs
}

fn session_files(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir.join("chats"))
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                        n.starts_with("session-") && (n.ends_with(".jsonl") || n.ends_with(".json"))
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A session file read back into its final state.
#[derive(Debug, Default)]
pub struct Record {
    pub meta: serde_json::Map<String, Value>,
    pub messages: Vec<Value>,
}

pub fn parse(text: &str) -> Result<Record> {
    let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let head: Value = serde_json::from_str(first).unwrap_or(Value::Null);
    // A legacy file is one JSON document (possibly pretty-printed).
    if head.is_null() || head.get("messages").is_some() {
        let doc: Value = serde_json::from_str(text).context("parse Gemini session")?;
        let mut meta = doc.as_object().cloned().unwrap_or_default();
        let messages = meta
            .remove("messages")
            .and_then(|m| m.as_array().cloned())
            .unwrap_or_default();
        return Ok(Record { meta, messages });
    }
    let mut r = Record::default();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(set) = v.get("$set").and_then(Value::as_object) {
            for (k, val) in set {
                if k == "messages" {
                    r.messages = val.as_array().cloned().unwrap_or_default();
                } else {
                    r.meta.insert(k.clone(), val.clone());
                }
            }
        } else if let Some(id) = v.get("$rewindTo").and_then(Value::as_str) {
            if let Some(i) = r.messages.iter().position(|m| m["id"] == id) {
                r.messages.truncate(i);
            }
        } else if v.get("type").is_some() && v.get("id").is_some() {
            match r.messages.iter().position(|m| m["id"] == v["id"]) {
                Some(i) => r.messages[i] = v,
                None => r.messages.push(v),
            }
        } else if let Some(o) = v.as_object() {
            for (k, val) in o {
                r.meta.insert(k.clone(), val.clone());
            }
        }
    }
    Ok(r)
}

fn read_record(path: &Path) -> Option<Record> {
    parse(&std::fs::read_to_string(path).ok()?).ok()
}

fn session_ref(path: PathBuf, r: &Record) -> Option<SessionRef> {
    if r.meta.get("kind").and_then(Value::as_str) == Some("subagent") {
        return None;
    }
    Some(SessionRef {
        tool: Tool::Gemini,
        id: r.meta.get("sessionId")?.as_str()?.to_string(),
        modified: modified(&path),
        title: r
            .meta
            .get("summary")
            .and_then(Value::as_str)
            .map(str::to_string),
        path: Some(path),
    })
}

pub fn list(cwd: &str) -> Result<Vec<SessionRef>> {
    let mut v: Vec<SessionRef> = project_dirs(cwd)
        .iter()
        .flat_map(|d| session_files(d))
        .filter_map(|p| {
            let r = read_record(&p)?;
            session_ref(p, &r)
        })
        .collect();
    v.sort_by_key(|s| std::cmp::Reverse(s.modified));
    Ok(v)
}

pub fn by_id(id: &str) -> Option<SessionRef> {
    let short = id.get(..8).unwrap_or(id);
    std::fs::read_dir(tmp_dir())
        .ok()?
        .flatten()
        .flat_map(|e| session_files(&e.path()))
        .filter(|p| p.to_string_lossy().contains(short))
        .find_map(|p| {
            let r = read_record(&p)?;
            session_ref(p, &r).filter(|s| s.id == id || s.id.starts_with(id))
        })
}

/// Text of a Gemini `PartListUnion`: a string, a part, or a list of parts.
fn parts_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .map(parts_text)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(o) => {
            if let Some(t) = o.get("text").and_then(Value::as_str) {
                t.to_string()
            } else if let Some(r) = o.get("functionResponse") {
                let resp = &r["response"];
                resp["output"]
                    .as_str()
                    .or(resp["error"].as_str())
                    .or(resp["content"].as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| resp.to_string())
            } else if o.contains_key("inlineData") || o.contains_key("fileData") {
                "[file]".into()
            } else {
                String::new()
            }
        }
        _ => String::new(),
    }
}

pub fn convert_record(r: &Record) -> (Builder, Meta) {
    let mut b = Builder::default();
    let mut meta = Meta {
        title: r
            .meta
            .get("summary")
            .and_then(Value::as_str)
            .map(str::to_string),
        cwd: r
            .meta
            .get("directories")
            .and_then(|d| d[0].as_str())
            .map(str::to_string),
        ..Default::default()
    };
    for m in &r.messages {
        let ts = m["timestamp"].as_str();
        meta.seen(ts);
        let content = if m["displayContent"].is_null() {
            parts_text(&m["content"])
        } else {
            parts_text(&m["displayContent"])
        };
        match m["type"].as_str().unwrap_or("") {
            "user" => {
                if !content.trim().is_empty() {
                    b.add("user", content, ts);
                }
            }
            "gemini" => {
                let i = b.add("agent", content, ts);
                if let Some(model) = m["model"].as_str() {
                    meta.models.insert(model.to_string());
                    b.set(i, "model_name", json!(model));
                }
                for t in m["thoughts"].as_array().into_iter().flatten() {
                    let subject = t["subject"].as_str().unwrap_or("");
                    let desc = t["description"].as_str().unwrap_or("");
                    let text = match (subject.is_empty(), desc.is_empty()) {
                        (false, false) => format!("{subject}: {desc}"),
                        (false, true) => subject.to_string(),
                        _ => desc.to_string(),
                    };
                    b.append_text(i, "reasoning_content", &text);
                }
                for c in m["toolCalls"].as_array().into_iter().flatten() {
                    let id = c["id"].as_str().unwrap_or("");
                    b.call(
                        i,
                        id,
                        c["name"].as_str().unwrap_or("tool"),
                        c["args"].clone(),
                    );
                    let mut out = parts_text(&c["result"]);
                    if out.is_empty() {
                        out = c["resultDisplay"].as_str().unwrap_or("").to_string();
                    }
                    if !out.is_empty() || !c["result"].is_null() {
                        b.observe(id, &out);
                    }
                }
                let t = &m["tokens"];
                if let Some(v) = (Usage {
                    prompt: t["input"].as_u64(),
                    completion: t["output"].as_u64(),
                    cached: t["cached"].as_u64(),
                })
                .to_value()
                {
                    b.set(i, "metrics", v);
                }
            }
            kind @ ("error" | "warning") if !content.trim().is_empty() => {
                let i = b.add("system", content, ts);
                b.set(i, "extra", json!({ "kind": kind }));
            }
            _ => {}
        }
    }
    (b, meta)
}

pub fn convert(s: &SessionRef) -> Result<Converted> {
    let path = s.path.as_ref().context("Gemini session without a file")?;
    let bytes = std::fs::read(path)?;
    let r = parse(&String::from_utf8_lossy(&bytes))?;
    let (b, meta) = convert_record(&r);
    let totals = b.totals();
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    Ok(Converted {
        tool: Tool::Gemini,
        session_id: s.id.clone(),
        agent_name: "gemini-cli",
        steps: b.finish(),
        totals,
        meta,
        native: vec![NativeFile {
            path: format!("native/{name}"),
            bytes,
            restore_to: Some(format!("~/.gemini/tmp/{{gemini_project}}/chats/{name}")),
        }],
        layout: "gemini-cli/chats-v1",
        resume: format!("gemini --resume {}", s.id),
    })
}

/// The `~/.gemini/tmp` folder name for `cwd` on this machine, registering
/// the project the way Gemini CLI does when it is new here.
pub fn project_for_restore(cwd: &str) -> Result<String> {
    let path = gemini_home().join("projects.json");
    let mut reg = registry();
    if let Some(map) = reg["projects"].as_object() {
        if let Some(s) = map
            .iter()
            .find(|(root, _)| same_dir(root, cwd))
            .and_then(|(_, s)| s.as_str())
        {
            return Ok(s.to_string());
        }
    }
    let base = slug(cwd);
    let taken = |s: &str| {
        let dir = tmp_dir().join(s);
        let owner = std::fs::read_to_string(dir.join(".project_root")).unwrap_or_default();
        (dir.exists() && !same_dir(owner.trim(), cwd))
            || reg["projects"]
                .as_object()
                .is_some_and(|m| m.values().any(|v| v == s))
    };
    let mut chosen = base.clone();
    let mut n = 1;
    while taken(&chosen) {
        chosen = format!("{base}-{n}");
        n += 1;
    }
    let root = if cfg!(windows) {
        cwd.to_lowercase()
    } else {
        cwd.to_string()
    };
    let dir = tmp_dir().join(&chosen);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(".project_root"), &root)?;
    if !reg["projects"].is_object() {
        reg["projects"] = json!({});
    }
    reg["projects"][root] = json!(chosen);
    std::fs::write(&path, serde_json::to_string_pretty(&reg)? + "\n")?;
    Ok(chosen)
}

#[cfg(test)]
mod tests {
    use super::*;

    const JSONL: &str = r#"{"sessionId":"1b2c3d4e-0000-4000-8000-000000000001","projectHash":"ab","startTime":"2026-09-20T10:00:00.000Z","lastUpdated":"2026-09-20T10:05:00.000Z","kind":"main"}
{"id":"m1","timestamp":"2026-09-20T10:00:01.000Z","type":"user","content":[{"text":"list the files"}]}
{"id":"m2","timestamp":"2026-09-20T10:00:02.000Z","type":"gemini","content":"","model":"gemini-2.5-pro"}
{"id":"m2","timestamp":"2026-09-20T10:00:03.000Z","type":"gemini","content":"Here they are.","model":"gemini-2.5-pro","thoughts":[{"subject":"Plan","description":"run ls","timestamp":"2026-09-20T10:00:02.500Z"}],"toolCalls":[{"id":"c1","name":"run_shell_command","args":{"command":"ls"},"result":[{"functionResponse":{"id":"c1","name":"run_shell_command","response":{"output":"a.txt\nb.txt"}}}],"status":"success","timestamp":"2026-09-20T10:00:02.600Z"}],"tokens":{"input":100,"output":20,"cached":50,"thoughts":5,"tool":0,"total":175}}
{"id":"m3","timestamp":"2026-09-20T10:01:00.000Z","type":"user","content":"never mind"}
{"$rewindTo":"m3"}
{"$set":{"summary":"List files","lastUpdated":"2026-09-20T10:05:00.000Z"}}
"#;

    #[test]
    fn jsonl_replays_updates_rewinds_and_sets() {
        let r = parse(JSONL).unwrap();
        assert_eq!(r.messages.len(), 2, "m2 replaced in place, m3 rewound");
        assert_eq!(r.meta["summary"], "List files");
        let (b, meta) = convert_record(&r);
        let steps = b.finish();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0]["message"], "list the files");
        let a = &steps[1];
        assert_eq!(a["message"], "Here they are.");
        assert_eq!(a["reasoning_content"], "Plan: run ls");
        assert_eq!(a["tool_calls"][0]["function_name"], "run_shell_command");
        assert_eq!(a["observation"]["results"][0]["content"], "a.txt\nb.txt");
        assert_eq!(a["metrics"]["prompt_tokens"], 100);
        assert_eq!(a["metrics"]["cached_tokens"], 50);
        assert!(meta.models.contains("gemini-2.5-pro"));
        assert_eq!(meta.title.as_deref(), Some("List files"));
    }

    #[test]
    fn legacy_json_document() {
        let doc = r#"{
  "sessionId": "s", "projectHash": "h", "startTime": "t", "lastUpdated": "t",
  "messages": [{"id": "a", "timestamp": "t", "type": "user", "content": "hi"}]
}"#;
        let r = parse(doc).unwrap();
        assert_eq!(r.meta["sessionId"], "s");
        assert_eq!(r.messages.len(), 1);
    }

    #[test]
    fn slug_follows_gemini() {
        assert_eq!(slug("/home/me/My Project!"), "my-project");
        assert_eq!(slug(r"C:\work\Project2"), "project2");
        assert_eq!(slug("/"), "project");
    }
}
