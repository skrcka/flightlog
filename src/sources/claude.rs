//! Claude Code: `~/.claude/projects/<cwd slug>/<session>.jsonl`, plus a
//! `<session>/` folder (subagents, large tool results).

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Value};

use super::{
    blocks_text, cwd_slug, home, modified, walk, Converted, Meta, NativeFile, SessionRef, Tool,
};
use crate::atif::{Builder, Usage};

pub fn projects_dir() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".claude"))
        .join("projects")
}

fn session_ref(path: PathBuf) -> SessionRef {
    SessionRef {
        tool: Tool::Claude,
        id: path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        modified: modified(&path),
        title: None,
        path: Some(path),
    }
}

/// The project folder for `cwd`; Windows drive letters may differ in case.
fn project_dir(cwd: &str) -> PathBuf {
    let slug = cwd_slug(cwd);
    let exact = projects_dir().join(&slug);
    if exact.is_dir() {
        return exact;
    }
    std::fs::read_dir(projects_dir())
        .ok()
        .and_then(|rd| {
            rd.flatten().map(|e| e.path()).find(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(&slug))
            })
        })
        .unwrap_or(exact)
}

pub fn list(cwd: &str) -> Result<Vec<SessionRef>> {
    let dir = project_dir(cwd);
    let mut v: Vec<SessionRef> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
                .map(session_ref)
                .collect()
        })
        .unwrap_or_default();
    v.sort_by_key(|s| std::cmp::Reverse(s.modified));
    Ok(v)
}

pub fn by_id(id: &str) -> Option<SessionRef> {
    let rd = std::fs::read_dir(projects_dir()).ok()?;
    rd.flatten()
        .map(|e| e.path().join(format!("{id}.jsonl")))
        .find(|p| p.is_file())
        .map(session_ref)
}

fn usage(u: &Value) -> Usage {
    Usage {
        prompt: u["input_tokens"].as_u64(),
        completion: u["output_tokens"].as_u64(),
        cached: u["cache_read_input_tokens"].as_u64(),
    }
}

pub fn convert_file(path: &Path) -> Result<(Builder, Meta)> {
    let f = std::io::Cursor::new(crate::safe_fs::read(path, crate::safe_fs::MAX_FILE_BYTES)?);
    let mut b = Builder::default();
    let mut meta = Meta::default();
    // One model turn spans several lines sharing message.id.
    let mut by_message: HashMap<String, usize> = HashMap::new();
    for line in BufReader::new(f).lines() {
        let Ok(line) = line else { continue };
        let Ok(d) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let t = d["type"].as_str().unwrap_or("");
        let ts = d["timestamp"].as_str();
        meta.seen(ts);
        if meta.cwd.is_none() {
            meta.cwd = d["cwd"].as_str().map(str::to_string);
        }
        if let Some(v) = d["version"].as_str() {
            meta.version = Some(v.to_string());
        }
        if t == "ai-title" {
            if let Some(title) = d["title"].as_str() {
                meta.title = Some(title.to_string());
            }
        }
        if t == "summary" || d["isCompactSummary"] == true {
            meta.compacted = true;
            let text = d["summary"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| blocks_text(&d["message"]["content"]));
            let i = b.add("system", text, ts);
            b.set(i, "is_copied_context", json!(true));
            b.set(i, "extra", json!({ "kind": "compaction_summary" }));
            continue;
        }
        if (t != "user" && t != "assistant") || d["isSidechain"] == true {
            continue;
        }
        let msg = &d["message"];
        let content = &msg["content"];
        if t == "user" {
            let mut text = String::new();
            if let Value::Array(blocks) = content {
                for blk in blocks {
                    if blk["type"] == "tool_result" {
                        let out = blocks_text(&blk["content"]);
                        b.observe(blk["tool_use_id"].as_str().unwrap_or(""), &out);
                    }
                }
                let rest: Vec<Value> = blocks
                    .iter()
                    .filter(|x| x["type"] != "tool_result")
                    .cloned()
                    .collect();
                text = blocks_text(&Value::Array(rest));
            } else if let Some(s) = content.as_str() {
                text = s.to_string();
            }
            if !text.trim().is_empty() {
                let meta_line = d["isMeta"] == true;
                let i = b.add(if meta_line { "system" } else { "user" }, text, ts);
                if meta_line {
                    b.set(i, "extra", json!({ "origin": "meta" }));
                }
            }
            continue;
        }
        let model = msg["model"].as_str();
        if let Some(m) = model.filter(|m| !m.starts_with('<')) {
            meta.models.insert(m.to_string());
        }
        let key = msg["id"]
            .as_str()
            .or(d["uuid"].as_str())
            .unwrap_or("")
            .to_string();
        let idx = *by_message.entry(key).or_insert_with(|| {
            let i = b.add("agent", "", ts);
            if let Some(m) = model {
                b.set(i, "model_name", json!(m));
            }
            i
        });
        let blocks: Vec<Value> = match content {
            Value::Array(a) => a.clone(),
            Value::String(s) => vec![json!({ "type": "text", "text": s })],
            _ => Vec::new(),
        };
        for blk in &blocks {
            match blk["type"].as_str() {
                Some("text") => b.append_text(idx, "message", blk["text"].as_str().unwrap_or("")),
                Some("thinking") => b.append_text(
                    idx,
                    "reasoning_content",
                    blk["thinking"].as_str().unwrap_or(""),
                ),
                Some("tool_use") => b.call(
                    idx,
                    blk["id"].as_str().unwrap_or(""),
                    blk["name"].as_str().unwrap_or("tool"),
                    blk["input"].clone(),
                ),
                _ => {}
            }
        }
        // The same usage repeats on every line of the turn: set, don't add.
        if let Some(m) = usage(&msg["usage"]).to_value() {
            b.set(idx, "metrics", m);
        }
    }
    Ok((b, meta))
}

pub fn convert(s: &SessionRef) -> Result<Converted> {
    let path = s
        .path
        .as_ref()
        .context("Claude Code session without a file")?;
    let (b, meta) = convert_file(path)?;
    let totals = b.totals();
    let mut native = vec![NativeFile {
        path: format!("native/{}.jsonl", s.id),
        bytes: crate::safe_fs::read(path, crate::safe_fs::MAX_FILE_BYTES)?,
        restore_to: Some(format!("~/.claude/projects/{{cwd_slug}}/{}.jsonl", s.id)),
    }];
    let side = path.with_extension("");
    let parent = side.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut total = native[0].bytes.len();
    for f in walk(&side) {
        total = total.saturating_add(std::fs::symlink_metadata(&f)?.len() as usize);
        if total > crate::safe_fs::MAX_FILE_BYTES {
            anyhow::bail!("native session exceeds size limit");
        }
        let rel = f
            .strip_prefix(&parent)
            .unwrap_or(&f)
            .to_string_lossy()
            .replace('\\', "/");
        native.push(NativeFile {
            path: format!("native/{rel}"),
            bytes: crate::safe_fs::read(&f, crate::safe_fs::MAX_FILE_BYTES)?,
            restore_to: Some(format!("~/.claude/projects/{{cwd_slug}}/{rel}")),
        });
    }
    Ok(Converted {
        tool: Tool::Claude,
        session_id: s.id.clone(),
        agent_name: "claude-code",
        steps: b.finish(),
        totals,
        meta,
        native,
        layout: "claude_code/projects-v1",
        resume: format!("claude --resume {}", s.id),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_a_turn_counts_usage_once_and_pairs_tool_results() {
        let lines = [
            r#"{"type":"user","timestamp":"t1","cwd":"/w","version":"2.5","message":{"role":"user","content":"fix the bug"}}"#,
            r#"{"type":"assistant","timestamp":"t2","message":{"id":"m1","model":"claude-x","content":[{"type":"thinking","thinking":"hmm"}],"usage":{"input_tokens":10,"output_tokens":5}}}"#,
            r#"{"type":"assistant","timestamp":"t2","message":{"id":"m1","model":"claude-x","content":[{"type":"text","text":"Looking."}],"usage":{"input_tokens":10,"output_tokens":5}}}"#,
            r#"{"type":"assistant","timestamp":"t2","message":{"id":"m1","model":"claude-x","content":[{"type":"tool_use","id":"tu1","name":"Bash","input":{"command":"ls"}}],"usage":{"input_tokens":10,"output_tokens":5}}}"#,
            r#"{"type":"user","timestamp":"t3","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu1","content":"a.rs"}]}}"#,
            r#"{"type":"assistant","timestamp":"t4","isSidechain":true,"message":{"id":"s","content":[{"type":"text","text":"subagent"}]}}"#,
            r#"{"type":"summary","summary":"earlier work"}"#,
        ];
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        std::fs::write(&p, lines.join("\n")).unwrap();
        let (b, meta) = convert_file(&p).unwrap();
        assert_eq!(b.totals(), (10, 5, 0), "usage counted once per turn");
        let steps = b.finish();
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0]["source"], "user");
        assert_eq!(steps[1]["message"], "Looking.");
        assert_eq!(steps[1]["reasoning_content"], "hmm");
        assert_eq!(steps[1]["tool_calls"][0]["function_name"], "Bash");
        assert_eq!(steps[1]["observation"]["results"][0]["content"], "a.rs");
        assert_eq!(steps[2]["is_copied_context"], true);
        assert!(meta.compacted);
        assert_eq!(meta.version.as_deref(), Some("2.5"));
        assert!(meta.models.contains("claude-x"));
    }
}
