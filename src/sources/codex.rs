//! Codex: `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<ts>-<session>.jsonl`.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use super::{home, modified, walk, Converted, Meta, NativeFile, SessionRef, Tool};
use crate::atif::{Builder, Usage};

pub fn codex_home() -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".codex"))
}

fn first_meta(p: &std::path::Path) -> Option<Value> {
    let f = std::fs::File::open(p).ok()?;
    let line = BufReader::new(f).lines().next()?.ok()?;
    let d: Value = serde_json::from_str(&line).ok()?;
    (d["type"] == "session_meta").then(|| d["payload"].clone())
}

fn rollouts() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = walk(&codex_home().join("sessions"))
        .into_iter()
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("rollout-") && n.ends_with(".jsonl"))
        })
        .collect();
    v.sort_by_key(|p| std::cmp::Reverse(modified(p)));
    v
}

fn session_ref(path: PathBuf, meta: &Value) -> SessionRef {
    let id = meta["id"]
        .as_str()
        .or(meta["session_id"].as_str())
        .map(str::to_string)
        .unwrap_or_else(|| {
            path.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
    SessionRef {
        tool: Tool::Codex,
        id,
        modified: modified(&path),
        title: None,
        path: Some(path),
    }
}

pub fn list(cwd: &str) -> Result<Vec<SessionRef>> {
    Ok(rollouts()
        .into_iter()
        .filter_map(|p| {
            let m = first_meta(&p)?;
            (m["cwd"] == cwd).then(|| session_ref(p, &m))
        })
        .collect())
}

pub fn by_id(id: &str) -> Option<SessionRef> {
    rollouts()
        .into_iter()
        .find(|p| p.to_string_lossy().contains(id))
        .map(|p| {
            let m = first_meta(&p).unwrap_or(Value::Null);
            session_ref(p, &m)
        })
}

pub fn convert(s: &SessionRef) -> Result<Converted> {
    let path = s.path.as_ref().context("Codex session without a file")?;
    let f = std::fs::File::open(path)?;
    let mut b = Builder::default();
    let mut meta = Meta::default();
    let mut last_agent: Option<usize> = None;
    for line in BufReader::new(f).lines() {
        let Ok(line) = line else { continue };
        let Ok(d) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let ts = d["timestamp"].as_str();
        meta.seen(ts);
        let p = &d["payload"];
        match d["type"].as_str().unwrap_or("") {
            "session_meta" => {
                meta.cwd = p["cwd"].as_str().map(str::to_string);
                meta.version = p["cli_version"].as_str().map(str::to_string);
            }
            "turn_context" => {
                if let Some(m) = p["model"].as_str() {
                    meta.models.insert(m.to_string());
                }
            }
            "compacted" => {
                meta.compacted = true;
                let i = b.add("system", p["message"].as_str().unwrap_or(""), ts);
                b.set(i, "is_copied_context", json!(true));
                b.set(i, "extra", json!({ "kind": "compaction_summary" }));
                last_agent = None;
            }
            "event_msg" if p["type"] == "token_count" => {
                let u = &p["info"]["last_token_usage"];
                if let (Some(i), Some(m)) = (
                    last_agent,
                    Usage {
                        prompt: u["input_tokens"].as_u64(),
                        completion: u["output_tokens"].as_u64(),
                        cached: u["cached_input_tokens"].as_u64(),
                    }
                    .to_value(),
                ) {
                    b.set(i, "metrics", m);
                }
            }
            "response_item" => match p["type"].as_str().unwrap_or("") {
                "message" => {
                    let text: String = p["content"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|c| c["text"].as_str())
                                .collect::<Vec<_>>()
                                .join("\n")
                        })
                        .unwrap_or_default();
                    if text.trim().is_empty() {
                        continue;
                    }
                    match p["role"].as_str() {
                        Some("assistant") => last_agent = Some(b.add("agent", text, ts)),
                        Some("user") => {
                            b.add("user", text, ts);
                            last_agent = None;
                        }
                        _ => {
                            b.add("system", text, ts);
                        }
                    }
                }
                "reasoning" => {
                    let summary: String = p["summary"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|s| s["text"].as_str())
                                .collect::<Vec<_>>()
                                .join("\n")
                        })
                        .unwrap_or_default();
                    if !summary.is_empty() {
                        let i = *last_agent.get_or_insert_with(|| b.add("agent", "", ts));
                        b.append_text(i, "reasoning_content", &summary);
                    }
                }
                kind @ ("function_call" | "custom_tool_call" | "local_shell_call") => {
                    let raw = if !p["arguments"].is_null() {
                        p["arguments"].clone()
                    } else if !p["input"].is_null() {
                        p["input"].clone()
                    } else {
                        p["action"].clone()
                    };
                    let args = match raw {
                        Value::String(s) => {
                            serde_json::from_str(&s).unwrap_or(json!({ "input": s }))
                        }
                        v => v,
                    };
                    let i = *last_agent.get_or_insert_with(|| b.add("agent", "", ts));
                    let id = p["call_id"].as_str().or(p["id"].as_str()).unwrap_or("");
                    b.call(i, id, p["name"].as_str().unwrap_or(kind), args);
                }
                "function_call_output" | "custom_tool_call_output" => {
                    let out = match &p["output"] {
                        Value::String(s) => s.clone(),
                        Value::Object(o) => o
                            .get("content")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .unwrap_or_else(|| p["output"].to_string()),
                        v => v.to_string(),
                    };
                    b.observe(p["call_id"].as_str().unwrap_or(""), &out);
                }
                _ => {}
            },
            _ => {}
        }
    }
    let totals = b.totals();
    let rel = path
        .strip_prefix(codex_home().join("sessions"))
        .map(|r| r.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| {
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    Ok(Converted {
        tool: Tool::Codex,
        session_id: s.id.clone(),
        agent_name: "codex",
        steps: b.finish(),
        totals,
        meta,
        native: vec![NativeFile {
            path: format!("native/{name}"),
            bytes: std::fs::read(path)?,
            restore_to: Some(format!("~/.codex/sessions/{rel}")),
        }],
        layout: "codex/rollout-v1",
        resume: format!("codex resume {}", s.id),
    })
}
