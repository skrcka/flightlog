//! Inside (task tracker) as a target, through its MCP endpoint:
//! `INSIDE_URL` + `INSIDE_MCP_TOKEN` (an `mcp_…` token).

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

pub struct Inside {
    base: String,
    token: String,
}

impl Inside {
    pub fn from_env(url: Option<String>) -> Result<Self> {
        let base = url
            .or_else(|| std::env::var("INSIDE_URL").ok())
            .context("set INSIDE_URL (e.g. https://inside.example.com) or pass --inside-url")?
            .trim_end_matches('/')
            .to_string();
        let token = std::env::var("INSIDE_MCP_TOKEN")
            .ok()
            .filter(|t| !t.is_empty())
            .context("set INSIDE_MCP_TOKEN (Inside → Settings → Personal access tokens (MCP))")?;
        Ok(Self { base, token })
    }

    fn call(&self, tool: &str, action: &str, mut args: Value) -> Result<Value> {
        args["action"] = json!(action);
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                           "params": { "name": tool, "arguments": args } });
        let resp: Value = match ureq::post(&format!("{}/api/mcp", self.base))
            .set("Authorization", &format!("Bearer {}", self.token))
            .send_json(body)
        {
            Ok(r) => r.into_json()?,
            Err(ureq::Error::Status(code, _)) => bail!("Inside {action}: HTTP {code}"),
            Err(e) => bail!("Inside {action}: {e}"),
        };
        if let Some(e) = resp.get("error").filter(|e| !e.is_null()) {
            bail!(
                "Inside {action}: {}",
                e["message"].as_str().unwrap_or("error")
            );
        }
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("null");
        let v: Value = serde_json::from_str(text).unwrap_or(Value::Null);
        if resp["result"]["isError"] == true {
            bail!("Inside {action}: {}", v["error"].as_str().unwrap_or(text));
        }
        Ok(v)
    }

    /// A task by key (`PROJ-42`) or UUID.
    pub fn task(&self, key: &str) -> Result<Value> {
        if uuid::Uuid::parse_str(key).is_ok() {
            self.call("projects_read", "get_task", json!({ "id": key }))
        } else {
            self.call(
                "projects_read",
                "get_task_by_key",
                json!({ "filters": { "key": key } }),
            )
        }
    }

    /// Attach a bundle to a task; returns the stored record.
    pub fn push(&self, task_key: &str, filename: &str, bytes: &[u8]) -> Result<Value> {
        let task = self.task(task_key)?;
        let task_id = task["id"].as_str().context("task without id")?;
        let mut data = json!({ "taskId": task_id, "filename": filename, "sizeBytes": bytes.len(),
                               "sha256": crate::bundle::sha256_hex(bytes) });
        if let Some(org) = task["orgId"].as_str() {
            data["orgId"] = json!(org);
        }
        let start = self.call(
            "projects_write",
            "start_task_context_upload",
            json!({ "data": data }),
        )?;
        let url = start["uploadUrl"].as_str().context("no uploadUrl")?;
        super::put(url, bytes)?;
        let id = start["id"].as_str().context("no id")?;
        let mut args = json!({ "id": id });
        if let Some(org) = task["orgId"].as_str() {
            args["data"] = json!({ "orgId": org });
        }
        self.call("projects_write", "complete_task_context_upload", args)
    }

    pub fn list(&self, task_key: &str) -> Result<Vec<Value>> {
        let task = self.task(task_key)?;
        let v = self.call(
            "projects_read",
            "list_task_contexts",
            json!({ "filters": { "taskId": task["id"] } }),
        )?;
        Ok(v["items"].as_array().cloned().unwrap_or_default())
    }

    pub fn download(&self, context_id: &str) -> Result<Vec<u8>> {
        let d = self.call(
            "projects_read",
            "get_task_context_download",
            json!({ "id": context_id }),
        )?;
        super::get(d["url"].as_str().context("no download url")?)
    }
}
