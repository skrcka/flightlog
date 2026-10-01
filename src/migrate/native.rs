//! Provider-specific destination encoders over the common historical messages.
use super::{history, row, Destination};
use crate::{
    bundle::Bundle,
    restore::Restored,
    safe_fs,
    sources::{self, claude, cursor, gemini},
};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn encode(value: &Value) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(value)?;
    if !crate::bypass::get().skip_size_checks && bytes.len() > safe_fs::MAX_FILE_BYTES {
        bail!("converted session exceeds size limit");
    }
    Ok(bytes)
}

pub(super) fn restore(bundle: &Bundle, to: Destination, cwd: &str) -> Result<Restored> {
    let messages = history(bundle)?;
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now();
    let ts = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let ms = now.timestamp_millis();
    match to {
        Destination::Copilot => {
            let mut bytes = Vec::new();
            let mut parent: Option<String> = None;
            let mut event = |kind: &str, data: Value| -> Result<()> {
                let eid = uuid::Uuid::new_v4().to_string();
                row(
                    &mut bytes,
                    json!({"id":eid,"parentId":parent,"timestamp":ts,"ephemeral":false,"type":kind,"data":data}),
                )?;
                parent = Some(eid);
                Ok(())
            };
            event(
                "session.start",
                json!({"sessionId":id,"version":1,"producer":"flightlog","copilotVersion":"unknown","startTime":ts,"context":{"cwd":cwd}}),
            )?;
            for (role, text) in messages {
                event(
                    if role == "user" {
                        "user.message"
                    } else {
                        "assistant.message"
                    },
                    json!({"content":text,"messageId":uuid::Uuid::new_v4().to_string()}),
                )?;
            }
            let path = sources::copilot::copilot_home()
                .join("session-state")
                .join(&id)
                .join("events.jsonl");
            safe_fs::write(&path, &bytes, false)?;
            Ok(Restored {
                written: vec![path],
                next: format!("copilot --resume {id}"),
            })
        }
        Destination::CopilotVscode => {
            let mut requests: Vec<Value> = Vec::new();
            for (role, text) in messages {
                if role == "user" {
                    requests.push(json!({"requestId":uuid::Uuid::new_v4().to_string(),"message":text,"variableData":{"variables":[]},"response":[],"timestamp":ms}));
                } else {
                    requests.last_mut().expect("history starts with handoff")["response"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!({"value":text,"isTrusted":false}));
                }
            }
            let value = json!({"initialLocation":"panel","responderUsername":"GitHub Copilot","requests":requests});
            let path = std::path::Path::new(".flightlog")
                .join("imports")
                .join(format!("{id}.copilot-vscode.json"));
            safe_fs::write(&path, &encode(&value)?, false)?;
            Ok(Restored {
                next: format!(
                    "In VS Code, run Chat: Import Chat from the Command Palette and select {}",
                    safe_fs::display(&path)
                ),
                written: vec![path],
            })
        }
        Destination::Claude => {
            let mut bytes = Vec::new();
            let mut parent: Option<String> = None;
            for (role, text) in messages {
                let uuid = uuid::Uuid::new_v4().to_string();
                let message = if role == "assistant" {
                    json!({"id":format!("msg_{uuid}"),"type":"message","role":role,"model":"unknown","content":[{"type":"text","text":text}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":0,"output_tokens":0}})
                } else {
                    json!({"role":"user","content":text})
                };
                row(
                    &mut bytes,
                    json!({"type":role,"uuid":uuid,"parentUuid":parent,"sessionId":id,"cwd":cwd,"timestamp":ts,"isSidechain":false,"userType":"external","version":"2.0.0","message":message}),
                )?;
                parent = Some(uuid);
            }
            let path = claude::projects_dir()
                .join(sources::cwd_slug(cwd))
                .join(format!("{id}.jsonl"));
            safe_fs::write(&path, &bytes, false)?;
            Ok(Restored {
                written: vec![path],
                next: format!("claude --resume {id}"),
            })
        }
        Destination::Gemini => {
            let mut bytes = Vec::new();
            row(
                &mut bytes,
                json!({"sessionId":id,"projectHash":gemini::project_hash(cwd),"startTime":ts,"lastUpdated":ts,"kind":"main","summary":"Imported Flightlog conversation"}),
            )?;
            for (role, text) in messages {
                row(
                    &mut bytes,
                    json!({"id":uuid::Uuid::new_v4().to_string(),"timestamp":ts,"type":if role=="assistant" {"gemini"} else {"user"},"content":text}),
                )?;
            }
            let project = gemini::project_for_restore(cwd)?;
            let path = gemini::gemini_home()
                .join("tmp")
                .join(project.name)
                .join("chats")
                .join(format!(
                    "session-{}-{}.jsonl",
                    now.format("%Y-%m-%dT%H-%M"),
                    &id[..8]
                ));
            for (p, _, force) in &project.writes {
                safe_fs::check_destination(p, *force)?;
            }
            safe_fs::check_destination(&path, false)?;
            safe_fs::write(&path, &bytes, false)?;
            for (p, bytes, force) in project.writes {
                safe_fs::write(&p, &bytes, force)?;
            }
            Ok(Restored {
                written: vec![path],
                next: format!("gemini --resume {id}"),
            })
        }
        Destination::Cursor => {
            let mut blobs = Vec::new();
            let mut tree = Vec::new();
            for (role, text) in messages {
                let content = json!({"role":role,"content":[{"type":"text","text":text}]});
                let hash = Sha256::digest(serde_json::to_vec(&content)?);
                let key = hash.iter().map(|b| format!("{b:02x}")).collect::<String>();
                tree.extend([0x0a, 0x20]);
                tree.extend_from_slice(&hash);
                if !blobs.iter().any(|b: &Value| b["id"] == key) {
                    blobs.push(json!({"id":key,"json":content}));
                }
            }
            let root = crate::bundle::sha256_hex(&tree);
            blobs.push(
                json!({"id":root,"hex":tree.iter().map(|b|format!("{b:02x}")).collect::<String>()}),
            );
            let dump = json!({"format":cursor::DUMP_FORMAT,"schema":[],"blobs":blobs,"store_meta":{"agentId":id,"latestRootBlobId":root,"name":"Imported Flightlog conversation"},"meta_json":{"schemaVersion":1,"createdAtMs":ms,"updatedAtMs":ms,"hasConversation":true,"cwd":cwd}});
            let path = cursor::restore_cli(&encode(&dump)?, &id, cwd, false)?;
            Ok(Restored {
                written: vec![path.join("store.db"), path.join("meta.json")],
                next: format!("cursor-agent --resume {id}"),
            })
        }
        Destination::Opencode => {
            let session_id = format!("ses_{}", id.replace('-', ""));
            let mut output = Vec::new();
            let mut parent = String::new();
            let first_ms = ms.saturating_sub(messages.len() as i64);
            for (index, (role, text)) in messages.into_iter().enumerate() {
                // opencode orders by timestamp and ID, not input array order.
                let message_ms = first_ms + index as i64;
                let mid = format!(
                    "msg_{message_ms:013x}{index:08x}{}",
                    uuid::Uuid::new_v4().simple()
                );
                let mut info = json!({"id":mid,"sessionID":session_id,"role":role,"time":{"created":message_ms}});
                if role == "assistant" {
                    info["parentID"] = json!(parent);
                    info["modelID"] = json!("unknown");
                    info["providerID"] = json!("unknown");
                    info["mode"] = json!("build");
                    info["agent"] = json!("build");
                    info["path"] = json!({"cwd":cwd,"root":cwd});
                    info["cost"] = json!(0);
                    info["tokens"] =
                        json!({"input":0,"output":0,"reasoning":0,"cache":{"read":0,"write":0}});
                    info["time"]["completed"] = json!(message_ms);
                    info["finish"] = json!("stop");
                } else {
                    parent = mid.clone();
                    info["agent"] = json!("build");
                    info["model"] = json!({"providerID":"unknown","modelID":"unknown"});
                }
                output.push(json!({"info":info,"parts":[{"id":format!("prt_{}",uuid::Uuid::new_v4().simple()),"sessionID":session_id,"messageID":mid,"type":"text","text":text}]}));
            }
            let value = json!({"info":{"id":session_id,"slug":format!("flightlog-{id}"),"projectID":"global","directory":cwd,"title":"Imported Flightlog conversation","version":env!("CARGO_PKG_VERSION"),"time":{"created":ms,"updated":ms}},"messages":output});
            let path = std::path::Path::new(".flightlog")
                .join("imports")
                .join(format!("{id}.opencode.json"));
            safe_fs::write(&path, &encode(&value)?, false)?;
            #[cfg(not(windows))]
            let command = format!(
                "opencode import {}",
                safe_fs::quote(&path.to_string_lossy())
            );
            #[cfg(windows)]
            let command = format!(
                "opencode import (file argument: {})",
                safe_fs::display(&path)
            );
            Ok(Restored {
                next: command,
                written: vec![path],
            })
        }
        Destination::Codex => unreachable!("handled by Codex adapter"),
    }
}
