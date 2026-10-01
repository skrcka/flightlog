//! Copilot CLI persisted events. Schema: github/copilot-sdk generated/session-events.ts.
use super::{home, modified, same_dir, walk, Converted, Meta, NativeFile, SessionRef, Tool};
use crate::{atif::Builder, safe_fs};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{collections::HashSet, path::PathBuf};

pub const LAYOUT: &str = "copilot-cli/events-v1";
pub fn copilot_home() -> PathBuf {
    std::env::var_os("COPILOT_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".copilot"))
}

pub fn parse(bytes: &[u8]) -> Result<Vec<Value>> {
    let mut events: Vec<Value> = Vec::new();
    for (line, raw) in std::str::from_utf8(bytes)?
        .lines()
        .filter(|s| !s.trim().is_empty())
        .enumerate()
    {
        let event: Value = serde_json::from_str(raw)
            .with_context(|| format!("invalid Copilot event line {}", line + 1))?;
        let kind = event["type"]
            .as_str()
            .context("missing Copilot event type")?;
        if !event["data"].is_object() {
            bail!("Copilot event data must be an object");
        }
        if kind == "session.snapshot_rewind" {
            let data = &event["data"];
            if let Some(ids) = data["eventIds"].as_array() {
                let ids: HashSet<_> = ids.iter().filter_map(Value::as_str).collect();
                events.retain(|e| !e["id"].as_str().is_some_and(|id| ids.contains(id)));
            } else {
                let id = data["upToEventId"]
                    .as_str()
                    .context("rewind missing target")?;
                let pos = events
                    .iter()
                    .position(|e| e["id"] == id)
                    .context("rewind target not found")?;
                events.truncate(pos);
            }
            continue;
        }
        if event["ephemeral"] == true {
            continue;
        }
        events.push(event);
    }
    let start = events.first().context("empty Copilot event log")?;
    if start["type"] != "session.start" || !start["data"]["sessionId"].is_string() {
        bail!("Copilot log must begin with session.start and sessionId");
    }
    Ok(events)
}

pub fn from_file(path: PathBuf) -> Result<SessionRef> {
    let events = parse(&safe_fs::read(&path, safe_fs::MAX_FILE_BYTES)?)?;
    let id = events[0]["data"]["sessionId"].as_str().unwrap();
    if !safe_fs::identifier(id) {
        bail!("invalid Copilot session id");
    }
    let title = events
        .iter()
        .rev()
        .find(|e| e["type"] == "session.title_changed")
        .and_then(|e| e["data"]["title"].as_str())
        .map(str::to_owned);
    Ok(SessionRef {
        tool: Tool::Copilot,
        id: id.into(),
        modified: modified(&path),
        title,
        path: Some(path),
    })
}

fn candidates() -> Vec<PathBuf> {
    walk(&copilot_home().join("session-state"))
        .into_iter()
        .filter(|p| {
            p.file_name().is_some_and(|n| n == "events.jsonl")
                || (p
                    .parent()
                    .is_some_and(|p| p == copilot_home().join("session-state"))
                    && p.extension().is_some_and(|s| s == "jsonl"))
        })
        .collect()
}
pub fn list(cwd: &str) -> Result<Vec<SessionRef>> {
    let mut found = Vec::new();
    for path in candidates() {
        let Ok(bytes) = safe_fs::read(&path, safe_fs::MAX_FILE_BYTES) else {
            continue;
        };
        let Ok(events) = parse(&bytes) else {
            continue;
        };
        if events[0]["data"]["context"]["cwd"]
            .as_str()
            .is_some_and(|dir| same_dir(dir, cwd))
        {
            if let Ok(s) = from_file(path) {
                found.push(s);
            }
        }
    }
    found.sort_by_key(|s| std::cmp::Reverse(s.modified));
    Ok(found)
}
pub fn by_id(id: &str) -> Option<SessionRef> {
    candidates()
        .into_iter()
        .filter_map(|p| from_file(p).ok())
        .find(|s| s.id == id)
}

pub fn convert(s: &SessionRef) -> Result<Converted> {
    let bytes = safe_fs::read(
        s.path.as_ref().context("missing Copilot path")?,
        safe_fs::MAX_FILE_BYTES,
    )?;
    let events = parse(&bytes)?;
    let start = &events[0]["data"];
    let mut meta = Meta {
        cwd: start["context"]["cwd"].as_str().map(str::to_owned),
        version: start["copilotVersion"].as_str().map(str::to_owned),
        title: s.title.clone(),
        ..Meta::default()
    };
    let mut b = Builder::default();
    let mut calls = HashSet::new();
    for event in &events {
        let kind = event["type"].as_str().unwrap();
        let data = &event["data"];
        let ts = event["timestamp"].as_str();
        meta.seen(ts);
        if let Some(model) = data["model"].as_str().or(data["selectedModel"].as_str()) {
            meta.models.insert(model.into());
        }
        match kind {
            "user.message" | "assistant.message" => {
                let content = data["content"]
                    .as_str()
                    .context("Copilot message missing content")?;
                let idx = b.add(
                    if kind == "user.message" {
                        "user"
                    } else {
                        "agent"
                    },
                    content,
                    ts,
                );
                if let Some(reasoning) = data["reasoningText"].as_str() {
                    b.set(idx, "reasoning_content", json!(reasoning));
                }
                if let Some(requests) = data["toolRequests"].as_array() {
                    for request in requests {
                        let id = request["toolCallId"]
                            .as_str()
                            .context("tool request missing id")?;
                        let name = request["name"]
                            .as_str()
                            .context("tool request missing name")?;
                        if calls.insert(id.to_string()) {
                            b.call(idx, id, name, request["arguments"].clone());
                        }
                    }
                }
                b.set(idx, "extra", json!({"copilot_event":event}));
            }
            "tool.execution_start" => {
                let id = data["toolCallId"]
                    .as_str()
                    .context("tool start missing id")?;
                if calls.insert(id.to_string()) {
                    let idx = b.add("agent", "", ts);
                    b.call(
                        idx,
                        id,
                        data["toolName"]
                            .as_str()
                            .context("tool start missing name")?,
                        data["arguments"].clone(),
                    );
                    b.set(idx, "extra", json!({"copilot_event":event}));
                }
            }
            "tool.execution_complete" => {
                let id = data["toolCallId"]
                    .as_str()
                    .context("tool result missing id")?;
                // JSON retains success/error, detailed output and structured results.
                b.observe(id, &serde_json::to_string(data)?);
            }
            _ => {
                let idx = b.add(
                    "system",
                    data["summaryContent"].as_str().unwrap_or(kind),
                    ts,
                );
                b.set(idx, "extra", json!({"copilot_event":event}));
                if kind == "session.compaction_complete" && data["success"] == true {
                    meta.compacted = true;
                    b.set(idx, "is_copied_context", json!(true));
                }
            }
        }
    }
    let totals = b.totals();
    Ok(Converted {
        tool: Tool::Copilot,
        session_id: s.id.clone(),
        agent_name: "GitHub Copilot CLI",
        steps: b.finish(),
        totals,
        meta,
        native: vec![NativeFile {
            path: format!("native/{}.jsonl", s.id),
            bytes,
            restore_to: None,
        }],
        layout: LAYOUT,
        resume: format!("copilot --resume {}", s.id),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rewinds_remove_selected_events_without_removing_interleaved_work() {
        let lines = [
            json!({"type":"session.start","data":{"sessionId":"test"},"id":"start"}),
            json!({"type":"user.message","data":{"content":"withdrawn"},"id":"one"}),
            json!({"type":"assistant.message","data":{"content":"retained"},"id":"two"}),
            json!({"type":"session.snapshot_rewind","ephemeral":true,"data":{"eventIds":["one"],"upToEventId":"one"}}),
        ];
        let bytes = lines
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        let events = parse(bytes.as_bytes()).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[1]["id"], "two");
        assert!(parse(b"{\"type\":\"user.message\",\"data\":{}}").is_err());
    }
}
