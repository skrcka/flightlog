//! VS Code chat JSON exports and persisted ObjectMutationLog JSONL.
//! Schema: microsoft/vscode src/vs/workbench/contrib/chat/common/model/{chatModel,objectMutationLog}.ts.
use super::{home, modified, same_dir, walk, Converted, Meta, NativeFile, SessionRef, Tool};
use crate::{atif::Builder, safe_fs};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::path::PathBuf;

pub const LAYOUT: &str = "copilot-vscode/chat-v1";

fn child<'a>(value: &'a mut Value, key: &Value) -> Result<&'a mut Value> {
    match key {
        Value::String(k) => value.get_mut(k).context("missing mutation property"),
        Value::Number(n) => value
            .get_mut(n.as_u64().context("invalid mutation index")? as usize)
            .context("mutation index out of bounds"),
        _ => bail!("invalid mutation key"),
    }
}

pub fn parse(bytes: &[u8]) -> Result<Value> {
    // Plain exports can span lines; mutation logs always begin with a kind record.
    if let Ok(v) = serde_json::from_slice::<Value>(bytes) {
        if v.get("requests").is_some() {
            return validate(v);
        }
    }
    let text = std::str::from_utf8(bytes).context("chat must be UTF-8")?;
    let mut state: Option<Value> = None;
    for (line, raw) in text.lines().filter(|s| !s.trim().is_empty()).enumerate() {
        let event: Value = serde_json::from_str(raw)
            .with_context(|| format!("invalid chat log line {}", line + 1))?;
        let kind = event["kind"].as_u64().context("missing mutation kind")?;
        if kind == 0 {
            state = Some(event.get("v").context("missing initial value")?.clone());
            continue;
        }
        let root = state.as_mut().context("mutation before initial state")?;
        let path = event["k"].as_array().context("missing mutation path")?;
        if path.len() > 128 {
            bail!("mutation path too deep");
        }
        if path.is_empty() {
            bail!("empty mutation path");
        }
        let mut parent = root;
        for key in &path[..path.len() - 1] {
            parent = child(parent, key)?;
        }
        let key = path.last().unwrap();
        match kind {
            1 | 3 => {
                let value = if kind == 3 {
                    Value::Null
                } else {
                    event.get("v").context("missing mutation value")?.clone()
                };
                if let Some(k) = key.as_str() {
                    let object = parent
                        .as_object_mut()
                        .context("mutation parent is not an object")?;
                    if kind == 3 {
                        object.remove(k);
                    } else {
                        object.insert(k.into(), value);
                    }
                } else {
                    *child(parent, key)? = value;
                }
            }
            2 => {
                if let Some(k) = key.as_str() {
                    let object = parent
                        .as_object_mut()
                        .context("mutation parent is not an object")?;
                    if object.get(k).is_none_or(Value::is_null) {
                        object.insert(k.into(), json!([]));
                    }
                }
                let array = child(parent, key)?
                    .as_array_mut()
                    .context("push target is not an array")?;
                if let Some(i) = event.get("i") {
                    let i = usize::try_from(i.as_u64().context("invalid splice index")?)?;
                    // Valid serializer diffs only truncate. Reject sparse-array allocation.
                    if i > array.len() {
                        bail!("splice index out of bounds");
                    }
                    array.truncate(i);
                }
                if let Some(v) = event.get("v") {
                    array.extend(
                        v.as_array()
                            .context("push value is not an array")?
                            .iter()
                            .cloned(),
                    );
                }
            }
            _ => bail!("unsupported mutation kind {kind}"),
        }
    }
    validate(state.context("empty chat log")?)
}

fn validate(v: Value) -> Result<Value> {
    if !v["requests"].is_array() || !v["responderUsername"].is_string() {
        bail!("expected a VS Code chat export (requests and responderUsername)");
    }
    Ok(v)
}

pub fn from_file(path: PathBuf) -> Result<SessionRef> {
    let bytes = safe_fs::read(&path, safe_fs::MAX_FILE_BYTES)?;
    let v = parse(&bytes)?;
    let id = v["sessionId"]
        .as_str()
        .filter(|id| safe_fs::identifier(id))
        .map(str::to_owned)
        .unwrap_or_else(|| crate::bundle::sha256_hex(&bytes)[..32].into());
    Ok(SessionRef {
        tool: Tool::CopilotVscode,
        id,
        modified: modified(&path),
        title: v["customTitle"]
            .as_str()
            .or(v["computedTitle"].as_str())
            .map(str::to_owned),
        path: Some(path),
    })
}

fn roots() -> Vec<PathBuf> {
    if let Some(root) = std::env::var_os("COPILOT_VSCODE_USER_DATA_DIR") {
        return vec![PathBuf::from(root)];
    }
    let base = if cfg!(target_os = "macos") {
        home().join("Library/Application Support")
    } else if cfg!(windows) {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join("AppData/Roaming"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(".config"))
    };
    ["Code", "Code - Insiders", "VSCodium"]
        .iter()
        .map(|name| base.join(name))
        .collect()
}

fn candidates() -> Vec<PathBuf> {
    roots()
        .iter()
        .flat_map(|root| walk(&root.join("User")))
        .filter(|p| {
            matches!(
                p.extension().and_then(|s| s.to_str()),
                Some("json" | "jsonl")
            ) && p
                .parent()
                .and_then(|p| p.file_name())
                .is_some_and(|s| s == "chatSessions" || s == "emptyWindowChatSessions")
        })
        .collect()
}

fn file_uri_path(s: &str) -> Option<String> {
    let url = s.strip_prefix("file://")?;
    if !url.starts_with('/') {
        return None;
    }
    let mut bytes = Vec::new();
    let mut chars = url.as_bytes().iter().copied();
    while let Some(c) = chars.next() {
        if c == b'%' {
            let a = (chars.next()? as char).to_digit(16)?;
            let b = (chars.next()? as char).to_digit(16)?;
            bytes.push((a * 16 + b) as u8);
        } else {
            bytes.push(c);
        }
    }
    let path = String::from_utf8(bytes).ok()?;
    if cfg!(windows) && path.as_bytes().get(2) == Some(&b':') {
        Some(path[1..].into())
    } else {
        Some(path)
    }
}

fn directory(v: &Value, path: &std::path::Path) -> Option<String> {
    if let Some(dir) = v["workingDirectory"].as_str().and_then(file_uri_path) {
        return Some(dir);
    }
    let workspace = path.parent()?.parent()?.join("workspace.json");
    let bytes = safe_fs::read(&workspace, safe_fs::MAX_FILE_BYTES).ok()?;
    let workspace: Value = serde_json::from_slice(&bytes).ok()?;
    workspace["folder"].as_str().and_then(file_uri_path)
}

pub fn list(cwd: &str) -> Result<Vec<SessionRef>> {
    let mut found = Vec::new();
    for path in candidates() {
        let Ok(bytes) = safe_fs::read(&path, safe_fs::MAX_FILE_BYTES) else {
            continue;
        };
        let Ok(v) = parse(&bytes) else {
            continue;
        };
        if directory(&v, &path).is_some_and(|dir| same_dir(&dir, cwd)) {
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
    let path = s.path.as_ref().context("missing chat path")?;
    let bytes = safe_fs::read(path, safe_fs::MAX_FILE_BYTES)?;
    let v = parse(&bytes)?;
    let mut b = Builder::default();
    let meta = Meta {
        cwd: directory(&v, path),
        title: s.title.clone(),
        ..Meta::default()
    };
    for request in v["requests"].as_array().unwrap() {
        let text = request["message"]
            .as_str()
            .or(request["message"]["text"].as_str())
            .context("chat request missing message text")?;
        let idx = b.add("user", text, None);
        let mut details = request
            .as_object()
            .context("chat request must be an object")?
            .clone();
        details.remove("message");
        details.remove("response");
        if request["message"].is_object() {
            details.insert("parsed_message".into(), request["message"].clone());
        }
        b.set(idx, "extra", json!({"vscode_request":details}));
        if let Some(response) = request.get("response").filter(|r| !r.is_null()) {
            let parts = response
                .as_array()
                .context("chat response must be an array")?;
            let text = parts
                .iter()
                .filter_map(|part| {
                    part.as_str()
                        .or(part["value"].as_str())
                        .or(part["content"]["value"].as_str())
                })
                .collect::<Vec<_>>()
                .join("\n");
            let text = if text.is_empty() && !parts.is_empty() {
                "[Non-text response; see recorded VS Code response details]".to_string()
            } else {
                text
            };
            let idx = b.add("agent", text, None);
            // Preserve tool records, references, and non-text response parts as data.
            b.set(idx, "extra", json!({"vscode_response":response}));
        }
    }
    let totals = b.totals();
    Ok(Converted {
        tool: Tool::CopilotVscode,
        session_id: s.id.clone(),
        agent_name: "GitHub Copilot (VS Code)",
        steps: b.finish(),
        totals,
        meta,
        native: vec![NativeFile {
            path: format!("native/{}.json", s.id),
            bytes: serde_json::to_vec(&v)?,
            restore_to: None,
        }],
        layout: LAYOUT,
        resume: "VS Code Command Palette: Chat: Import Chat".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reconstructs_chat_mutations_and_rejects_sparse_allocations() {
        let lines = [
            json!({"kind":0,"v":{"requests":[],"responderUsername":"Copilot"}}),
            json!({"kind":2,"k":["requests"],"v":[{"message":"first"},{"message":"discard"}]}),
            json!({"kind":2,"k":["requests"],"i":1,"v":[{"message":"second","hidden":true}]}),
            json!({"kind":1,"k":["requests",0,"message"],"v":"edited"}),
            json!({"kind":3,"k":["requests",1,"hidden"]}),
        ];
        let raw = lines
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        let v = parse(raw.as_bytes()).unwrap();
        assert_eq!(
            v["requests"],
            json!([{"message":"edited"},{"message":"second"}])
        );
        assert!(parse(
            format!(
                "{raw}\n{}",
                json!({"kind":2,"k":["requests"],"i":999999999})
            )
            .as_bytes()
        )
        .is_err());
        assert!(parse(b"{\"kind\":1,\"k\":[\"requests\"],\"v\":[]}").is_err());
    }
}
