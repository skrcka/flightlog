//! The flightlog bundle (SPEC.md): a ZIP with `manifest.json`,
//! `trajectory.json` (ATIF) and optionally `native/…` files for resume.

use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::atif;
use crate::redact::{self, Redactor};
use crate::sources::Converted;

pub const FORMAT: &str = "flightlog";
/// Earlier name of the same format, still accepted on read.
pub const FORMAT_LEGACY: &str = "inside.llm-context";
pub const FORMAT_VERSION: &str = "1.0";

pub const MAX_ARCHIVE_BYTES: usize = 100 * 1024 * 1024;
const MAX_UNCOMPRESSED: u64 = 1024 * 1024 * 1024;
const MAX_ENTRIES: usize = 10_000;

pub fn sha256_hex(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn git(args: &[&str], cwd: &str) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !s.is_empty()).then_some(s)
}

/// Options for building a bundle.
pub struct Build<'a> {
    pub summary: Value,
    pub include_native: bool,
    pub reviewed: bool,
    pub cwd: &'a str,
}

/// The built archive plus what the user must see before sharing it.
pub struct Built {
    pub bytes: Vec<u8>,
    pub findings: Vec<redact::Finding>,
}

fn compact(v: Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.into_iter()
                .filter(|(_, x)| !(x.is_null() || x == &json!([]) || x == &json!({})))
                .collect(),
        ),
        other => other,
    }
}

pub fn build(c: &Converted, opts: Build) -> Result<Built> {
    let mut red = Redactor::default();
    let steps: Vec<Value> = c
        .steps
        .iter()
        .map(|s| red.value(s, "trajectory.json"))
        .collect();
    let summary = red.value(&opts.summary, "manifest.json");
    let native: Vec<(String, Vec<u8>, Option<String>)> = if opts.include_native {
        c.native
            .iter()
            .map(|f| {
                let text_like = [".jsonl", ".json", ".txt", ".md"]
                    .iter()
                    .any(|x| f.path.ends_with(x));
                let bytes = match (text_like, std::str::from_utf8(&f.bytes)) {
                    (true, Ok(s)) => red.text(s, &f.path).into_bytes(),
                    _ => f.bytes.clone(),
                };
                (f.path.clone(), bytes, f.restore_to.clone())
            })
            .collect()
    } else {
        Vec::new()
    };

    let (prompt, completion, cached) = c.totals;
    let mut agent = json!({ "name": c.agent_name, "version": c.meta.version.clone().unwrap_or_else(|| "unknown".into()) });
    if let Some(m) = c.meta.models.iter().next() {
        agent["model_name"] = json!(m);
    }
    let trajectory = json!({
        "schema_version": atif::SCHEMA_VERSION,
        "session_id": c.session_id,
        "agent": agent,
        "steps": steps,
        "final_metrics": {
            "total_prompt_tokens": prompt,
            "total_completion_tokens": completion,
            "total_cached_tokens": cached,
            "total_steps": steps.len(),
        },
    });
    let traj = serde_json::to_vec(&trajectory)?;

    let mut files = vec![
        json!({ "path": "trajectory.json", "sha256": sha256_hex(&traj), "bytes": traj.len(), "role": "trajectory" }),
    ];
    for (p, b, _) in &native {
        files.push(
            json!({ "path": p, "sha256": sha256_hex(b), "bytes": b.len(), "role": "native" }),
        );
    }
    let cwd = c.meta.cwd.clone().unwrap_or_else(|| opts.cwd.to_string());
    let repository = compact(json!({
        "remote": git(&["remote", "get-url", "origin"], &cwd),
        "branch": git(&["rev-parse", "--abbrev-ref", "HEAD"], &cwd),
        "commit": git(&["rev-parse", "--short", "HEAD"], &cwd),
    }));
    let source = compact(json!({
        "tool": c.tool.id(),
        "tool_version": c.meta.version,
        "session_id": c.session_id,
        "title": c.meta.title,
        "models": c.meta.models.iter().collect::<Vec<_>>(),
        "started_at": c.meta.started,
        "ended_at": c.meta.ended,
        "cwd": cwd,
        "repository": repository,
        "compacted": c.meta.compacted,
    }));
    let mut manifest = json!({
        "format": FORMAT,
        "format_version": FORMAT_VERSION,
        "bundle_id": uuid::Uuid::new_v4().to_string(),
        "created_at": now(),
        "producer": { "name": "flightlog", "version": env!("CARGO_PKG_VERSION") },
        "source": source,
        "summary": summary,
        "redaction": {
            "engine": format!("flightlog/{}", env!("CARGO_PKG_VERSION")),
            "applied_at": now(),
            "reviewed_by_user": opts.reviewed,
            "native": if native.is_empty() { "absent" } else { "redacted" },
            "findings": red.findings_json(),
        },
        "stats": {
            "steps": steps.len(),
            "user_messages": steps.iter().filter(|s| s["source"] == "user").count(),
            "tool_calls": steps.iter().map(|s| s["tool_calls"].as_array().map_or(0, Vec::len)).sum::<usize>(),
            "total_prompt_tokens": prompt,
            "total_completion_tokens": completion,
        },
        "files": files,
    });
    if !native.is_empty() {
        manifest["native"] = json!({
            "tool": c.tool.id(),
            "layout": c.layout,
            "root": "native/",
            "entries": native.iter().filter_map(|(p, _, r)| r.as_ref().map(|r| json!({ "path": p, "restore_to": r }))).collect::<Vec<_>>(),
            "resume_command": c.resume,
        });
    }

    let mut buf = Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        let o = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o644);
        z.start_file("manifest.json", o)?;
        z.write_all(&serde_json::to_vec_pretty(&manifest)?)?;
        z.start_file("trajectory.json", o)?;
        z.write_all(&traj)?;
        for (p, b, _) in &native {
            z.start_file(p.as_str(), o)?;
            z.write_all(b)?;
        }
        z.finish()?;
    }
    Ok(Built {
        bytes: buf.into_inner(),
        findings: red.findings(),
    })
}

// ─── Reading and validation (SPEC.md §7) ─────────────────────────────────────

/// An opened, validated bundle.
#[derive(Debug)]
pub struct Bundle {
    pub manifest: Value,
    pub trajectory: Value,
    pub files: BTreeMap<String, Vec<u8>>,
}

fn safe_path(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('/')
        && !name.contains('\\')
        && !name.contains(':')
        && !name.split('/').any(|s| s == ".." || s == ".")
}

/// Open and validate. `Err` lists every problem found.
pub fn open(bytes: &[u8]) -> std::result::Result<Bundle, Vec<String>> {
    let mut problems = Vec::new();
    if bytes.len() > MAX_ARCHIVE_BYTES {
        return Err(vec![format!("archive exceeds {MAX_ARCHIVE_BYTES} bytes")]);
    }
    let mut z = match zip::ZipArchive::new(Cursor::new(bytes)) {
        Ok(z) => z,
        Err(e) => return Err(vec![format!("not a ZIP archive: {e}")]),
    };
    if z.len() > MAX_ENTRIES {
        return Err(vec![format!("more than {MAX_ENTRIES} entries")]);
    }
    let mut total = 0u64;
    let mut files = BTreeMap::new();
    for i in 0..z.len() {
        let mut e = match z.by_index(i) {
            Ok(e) => e,
            Err(err) => {
                problems.push(format!("entry {i}: {err}"));
                continue;
            }
        };
        let name = e.name().to_string();
        if !safe_path(&name) {
            problems.push(format!("unsafe path: {name}"));
            continue;
        }
        if e.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
            problems.push(format!("symlink: {name}"));
            continue;
        }
        if e.is_dir() {
            continue;
        }
        total = total.saturating_add(e.size());
        if total > MAX_UNCOMPRESSED {
            return Err(vec!["uncompressed content exceeds 1 GiB".into()]);
        }
        let mut buf = Vec::with_capacity(e.size() as usize);
        if e.read_to_end(&mut buf).is_err() {
            problems.push(format!("unreadable entry: {name}"));
            continue;
        }
        files.insert(name, buf);
    }
    let Some(mbytes) = files.get("manifest.json") else {
        problems.push("manifest.json is missing".into());
        return Err(problems);
    };
    let manifest: Value = match serde_json::from_slice(mbytes) {
        Ok(v) => v,
        Err(e) => {
            problems.push(format!("manifest.json: {e}"));
            return Err(problems);
        }
    };
    if manifest["format"] != FORMAT && manifest["format"] != FORMAT_LEGACY {
        problems.push(format!("manifest.format must be \"{FORMAT}\""));
    }
    if manifest["format_version"]
        .as_str()
        .and_then(|v| v.split('.').next())
        != Some("1")
    {
        problems.push("manifest.format_version must be 1.x".into());
    }
    for f in ["bundle_id", "created_at"] {
        if manifest[f].as_str().is_none_or(str::is_empty) {
            problems.push(format!("manifest.{f} is required"));
        }
    }
    for f in ["tool", "session_id"] {
        if manifest["source"][f].as_str().is_none_or(str::is_empty) {
            problems.push(format!("manifest.source.{f} is required"));
        }
    }
    for f in ["goal", "state"] {
        if manifest["summary"][f]
            .as_str()
            .is_none_or(|s| s.trim().is_empty())
        {
            problems.push(format!("manifest.summary.{f} is required"));
        }
    }
    if !manifest["redaction"]["findings"].is_array() {
        problems.push("manifest.redaction.findings is required".into());
    }
    let listed = manifest["files"].as_array().cloned().unwrap_or_default();
    if listed.is_empty() {
        problems.push("manifest.files is required".into());
    }
    for f in &listed {
        let Some(p) = f["path"].as_str() else {
            problems.push("manifest.files entry without path".into());
            continue;
        };
        match files.get(p) {
            None => problems.push(format!("listed file missing: {p}")),
            Some(b) if f["sha256"].as_str() != Some(sha256_hex(b).as_str()) => {
                problems.push(format!("checksum mismatch: {p}"))
            }
            _ => {}
        }
    }
    let trajectory: Value = match files
        .get("trajectory.json")
        .map(|b| serde_json::from_slice(b))
    {
        Some(Ok(v)) => v,
        Some(Err(e)) => {
            problems.push(format!("trajectory.json: {e}"));
            Value::Null
        }
        None => {
            problems.push("trajectory.json is missing".into());
            Value::Null
        }
    };
    if !trajectory["schema_version"]
        .as_str()
        .is_some_and(|v| v.starts_with("ATIF-v1."))
    {
        problems.push("trajectory.schema_version must be ATIF-v1.x".into());
    }
    if trajectory["session_id"] != manifest["source"]["session_id"] {
        problems.push("trajectory.session_id must equal manifest.source.session_id".into());
    }
    if !trajectory["steps"].is_array() {
        problems.push("trajectory.steps must be an array".into());
    }
    if let Some(t) = files.get("trajectory.json") {
        let mut text = String::from_utf8_lossy(t).into_owned();
        text.push_str(&manifest["summary"].to_string());
        for (kind, n) in redact::scan(&text) {
            problems.push(format!("unredacted secret: {n} × {kind}"));
        }
    }
    if problems.is_empty() {
        Ok(Bundle {
            manifest,
            trajectory,
            files,
        })
    } else {
        Err(problems)
    }
}

pub fn read(path: &std::path::Path) -> Result<Bundle> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    match open(&bytes) {
        Ok(b) => Ok(b),
        Err(p) => bail!(
            "invalid bundle {}:\n  - {}",
            path.display(),
            p.join("\n  - ")
        ),
    }
}

/// Write every file of the bundle under `dir`.
pub fn extract(b: &Bundle, dir: &std::path::Path) -> Result<()> {
    for (name, bytes) in &b.files {
        let p = dir.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&p, bytes)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::{Meta, NativeFile, Tool};

    fn converted(message: &str) -> Converted {
        Converted {
            tool: Tool::Codex,
            session_id: "s-1".into(),
            agent_name: "codex",
            steps: vec![
                json!({ "step_id": 1, "source": "user", "message": "hi" }),
                json!({ "step_id": 2, "source": "agent", "message": message }),
            ],
            totals: (1, 2, 0),
            meta: Meta {
                cwd: Some("/w".into()),
                ..Default::default()
            },
            native: vec![NativeFile {
                path: "native/rollout-s-1.jsonl".into(),
                bytes: format!("{{\"m\":\"{message}\"}}").into_bytes(),
                restore_to: Some("~/.codex/sessions/rollout-s-1.jsonl".into()),
            }],
            layout: "codex/rollout-v1",
            resume: "codex resume s-1".into(),
        }
    }

    fn opts() -> Build<'static> {
        Build {
            summary: json!({ "goal": "g", "state": "s" }),
            include_native: true,
            reviewed: true,
            cwd: "/w",
        }
    }

    #[test]
    fn built_bundles_validate_and_are_redacted_everywhere() {
        let c = converted("db postgres://app:hunter22@db/x");
        let built = build(&c, opts()).unwrap();
        assert_eq!(built.findings.len(), 1);
        let b = open(&built.bytes).expect("valid");
        let native = String::from_utf8(b.files["native/rollout-s-1.jsonl"].clone()).unwrap();
        assert!(native.contains("[REDACTED:password:1]"), "{native}");
        assert_eq!(b.manifest["native"]["resume_command"], "codex resume s-1");
        assert_eq!(b.trajectory["steps"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn tampering_and_missing_fields_are_reported() {
        let built = build(&converted("ok"), opts()).unwrap();
        // Re-pack with a changed trajectory and no summary goal.
        let mut z = zip::ZipArchive::new(Cursor::new(built.bytes)).unwrap();
        let mut manifest: Value =
            serde_json::from_reader(z.by_name("manifest.json").unwrap()).unwrap();
        manifest["summary"] = json!({ "state": "s" });
        let mut buf = Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let o = zip::write::SimpleFileOptions::default();
            w.start_file("manifest.json", o).unwrap();
            w.write_all(&serde_json::to_vec(&manifest).unwrap())
                .unwrap();
            w.start_file("trajectory.json", o).unwrap();
            w.write_all(b"{\"schema_version\":\"ATIF-v1.8\",\"session_id\":\"s-1\",\"steps\":[]}")
                .unwrap();
            w.start_file("../evil", o).unwrap();
            w.write_all(b"x").unwrap();
            w.finish().unwrap();
        }
        let problems = open(&buf.into_inner()).unwrap_err().join(" | ");
        assert!(problems.contains("unsafe path"), "{problems}");
        assert!(problems.contains("summary.goal"), "{problems}");
        assert!(
            problems.contains("checksum mismatch: trajectory.json"),
            "{problems}"
        );
    }
}
