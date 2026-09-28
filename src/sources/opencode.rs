//! opencode: sessions live in its SQLite database; `opencode export <id>`
//! prints one as JSON, `opencode import <file>` reads it back.

use std::io::{Read, Seek};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use super::{Converted, Meta, NativeFile, SessionRef, Tool};
use crate::atif::{Builder, Usage};

/// The opencode command: npm installs it as `opencode.cmd` on Windows,
/// which `Command` does not find under the bare name.
fn opencode() -> Command {
    static PROGRAM: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
    let program = PROGRAM.get_or_init(|| {
        let works = |p: &str| {
            Command::new(p)
                .arg("--version")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        };
        if cfg!(windows) && !works("opencode") && works("opencode.cmd") {
            "opencode.cmd"
        } else {
            "opencode"
        }
    });
    Command::new(program)
}

fn available() -> bool {
    opencode()
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

pub fn list(cwd: &str) -> Result<Vec<SessionRef>> {
    if !available() {
        return Ok(Vec::new());
    }
    let out = opencode()
        .args(["session", "list", "--format", "json"])
        .current_dir(cwd)
        .stderr(Stdio::null())
        .output()?;
    let sessions: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Array(vec![]));
    Ok(sessions
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| {
                    let updated = s["updated"].as_u64().or(s["time"]["updated"].as_u64());
                    Some(SessionRef {
                        tool: Tool::Opencode,
                        id: s["id"].as_str()?.to_string(),
                        path: None,
                        modified: updated
                            .map(|ms| std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms)),
                        title: s["title"].as_str().map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default())
}

/// `opencode export` into a file: its stdout is cut off when read through a
/// pipe (observed at ~60 KB).
pub fn export(id: &str) -> Result<String> {
    let mut tmp = tempfile_in_temp()?;
    let status = opencode()
        .args(["export", id])
        .stdout(tmp.try_clone()?)
        .stderr(Stdio::null())
        .status()
        .context("run opencode export")?;
    if !status.success() {
        bail!("opencode export {id} failed");
    }
    tmp.rewind()?;
    let mut raw = String::new();
    tmp.take(crate::bypass::limit(
        crate::safe_fs::MAX_FILE_BYTES as u64 + 1,
    ))
    .read_to_string(&mut raw)?;
    if !crate::bypass::get().skip_size_checks && raw.len() > crate::safe_fs::MAX_FILE_BYTES {
        bail!("session exceeds size limit");
    }
    let start = raw.find('{').context("opencode export printed no JSON")?;
    Ok(raw[start..].to_string())
}

fn tempfile_in_temp() -> Result<std::fs::File> {
    Ok(tempfile::tempfile()?)
}

pub fn convert_json(text: &str) -> Result<(Builder, Meta)> {
    let data: Value = serde_json::from_str(text).context("parse opencode export")?;
    let info = &data["info"];
    let mut b = Builder::default();
    let mut meta = Meta {
        cwd: info["directory"].as_str().map(str::to_string),
        title: info["title"].as_str().map(str::to_string),
        version: info["version"].as_str().map(str::to_string),
        ..Default::default()
    };
    for m in data["messages"].as_array().into_iter().flatten() {
        let mi = &m["info"];
        let parts = m["parts"].as_array().cloned().unwrap_or_default();
        let ts = mi["time"]["created"]
            .as_i64()
            .and_then(chrono::DateTime::from_timestamp_millis)
            .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
        meta.seen(ts.as_deref());
        if let Some(model) = mi["modelID"].as_str() {
            meta.models.insert(model.to_string());
        }
        let text: String = parts
            .iter()
            .filter(|p| p["type"] == "text")
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if mi["role"] == "user" {
            if !text.trim().is_empty() {
                b.add("user", text, ts.as_deref());
            }
            continue;
        }
        let i = b.add("agent", text, ts.as_deref());
        if let Some(model) = mi["modelID"].as_str() {
            b.set(i, "model_name", json!(model));
        }
        for p in &parts {
            match p["type"].as_str() {
                Some("reasoning") => {
                    b.append_text(i, "reasoning_content", p["text"].as_str().unwrap_or(""))
                }
                Some("tool") => {
                    let state = &p["state"];
                    let id = p["callID"].as_str().or(p["id"].as_str()).unwrap_or("");
                    b.call(
                        i,
                        id,
                        p["tool"].as_str().unwrap_or("tool"),
                        state["input"].clone(),
                    );
                    let out = state["output"]
                        .as_str()
                        .or(state["error"].as_str())
                        .unwrap_or("");
                    b.observe(id, out);
                }
                _ => {}
            }
        }
        let tok = &mi["tokens"];
        if let Some(v) = (Usage {
            prompt: tok["input"].as_u64(),
            completion: tok["output"].as_u64(),
            cached: tok["cache"]["read"].as_u64(),
        })
        .to_value()
        {
            b.set(i, "metrics", v);
        }
    }
    Ok((b, meta))
}

pub fn convert(s: &SessionRef) -> Result<Converted> {
    let text = export(&s.id)?;
    let (b, meta) = convert_json(&text)?;
    let totals = b.totals();
    Ok(Converted {
        tool: Tool::Opencode,
        session_id: s.id.clone(),
        agent_name: "opencode",
        steps: b.finish(),
        totals,
        meta,
        native: vec![NativeFile {
            path: format!("native/{}.json", s.id),
            bytes: text.into_bytes(),
            restore_to: None,
        }],
        layout: "opencode/export-v1",
        resume: "opencode import <file>".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_an_export() {
        let export = r#"{"info":{"directory":"/w","title":"T","version":"1.2"},"messages":[
          {"info":{"role":"user","time":{"created":1790000000000}},"parts":[{"type":"text","text":"hi"}]},
          {"info":{"role":"assistant","modelID":"m","time":{"created":1790000001000},"tokens":{"input":7,"output":3,"cache":{"read":2}}},
           "parts":[{"type":"reasoning","text":"think"},{"type":"text","text":"done"},
                    {"type":"tool","tool":"bash","callID":"c1","state":{"input":{"cmd":"ls"},"output":"x"}}]}]}"#;
        let (b, meta) = convert_json(export).unwrap();
        assert_eq!(b.totals(), (7, 3, 2));
        let steps = b.finish();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[1]["tool_calls"][0]["function_name"], "bash");
        assert_eq!(steps[1]["observation"]["results"][0]["content"], "x");
        assert_eq!(meta.title.as_deref(), Some("T"));
    }
}
