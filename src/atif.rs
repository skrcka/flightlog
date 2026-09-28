//! Building ATIF v1.8 trajectories (Harbor RFC 0001).
//!
//! Steps are kept as JSON values: ATIF is open-ended (`extra` objects) and
//! the converters fill different subsets of it.

use std::collections::HashMap;

use serde_json::{json, Map, Value};

pub const SCHEMA_VERSION: &str = "ATIF-v1.8";
/// Tool outputs larger than this are truncated (spec §3.5).
pub const MAX_OBSERVATION: usize = 256 * 1024;

/// Accumulates steps and routes tool results to the step that made the call.
#[derive(Default)]
pub struct Builder {
    pub steps: Vec<Value>,
    pending: HashMap<String, usize>,
}

/// Token usage of one model turn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub prompt: Option<u64>,
    pub completion: Option<u64>,
    pub cached: Option<u64>,
}

impl Usage {
    pub fn to_value(self) -> Option<Value> {
        let mut m = Map::new();
        if let Some(v) = self.prompt {
            m.insert("prompt_tokens".into(), json!(v));
        }
        if let Some(v) = self.completion {
            m.insert("completion_tokens".into(), json!(v));
        }
        if let Some(v) = self.cached {
            m.insert("cached_tokens".into(), json!(v));
        }
        (!m.is_empty()).then_some(Value::Object(m))
    }
}

/// Cut `text` to [`MAX_OBSERVATION`] bytes on a char boundary.
pub fn clip(text: &str) -> (String, Option<Value>) {
    if text.len() <= MAX_OBSERVATION {
        return (text.to_string(), None);
    }
    let mut end = MAX_OBSERVATION;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (
        text[..end].to_string(),
        Some(json!({ "truncated": true, "original_bytes": text.len() })),
    )
}

impl Builder {
    /// Append a step; returns its index. Empty optional fields are omitted.
    pub fn add(
        &mut self,
        source: &str,
        message: impl Into<String>,
        timestamp: Option<&str>,
    ) -> usize {
        let mut step = Map::new();
        step.insert("step_id".into(), json!(self.steps.len() + 1));
        if let Some(ts) = timestamp {
            step.insert("timestamp".into(), json!(ts));
        }
        step.insert("source".into(), json!(source));
        step.insert("message".into(), json!(message.into()));
        self.steps.push(Value::Object(step));
        self.steps.len() - 1
    }

    pub fn set(&mut self, idx: usize, key: &str, value: Value) {
        if let Some(Value::Object(m)) = self.steps.get_mut(idx) {
            m.insert(key.into(), value);
        }
    }

    pub fn append_text(&mut self, idx: usize, key: &str, text: &str) {
        if text.is_empty() {
            return;
        }
        if let Some(Value::Object(m)) = self.steps.get_mut(idx) {
            let joined = match m.get(key).and_then(Value::as_str) {
                Some(prev) if !prev.is_empty() => format!("{prev}\n{text}"),
                _ => text.to_string(),
            };
            m.insert(key.into(), json!(joined));
        }
    }

    /// Record a tool call on step `idx`.
    pub fn call(&mut self, idx: usize, id: &str, name: &str, arguments: Value) {
        if let Some(Value::Object(m)) = self.steps.get_mut(idx) {
            let calls = m.entry("tool_calls").or_insert_with(|| json!([]));
            if let Value::Array(a) = calls {
                a.push(
                    json!({ "tool_call_id": id, "function_name": name, "arguments": arguments }),
                );
            }
        }
        self.pending.insert(id.to_string(), idx);
    }

    /// Attach a tool result to the step that made the call (or a system step
    /// of its own when the call is unknown).
    pub fn observe(&mut self, call_id: &str, content: &str) {
        let (text, extra) = clip(content);
        let mut result = json!({ "source_call_id": call_id, "content": text });
        if let Some(e) = extra {
            result["extra"] = e;
        }
        let idx = match self.pending.get(call_id) {
            Some(i) => *i,
            None => {
                let i = self.add("system", "", None);
                self.set(i, "extra", json!({ "origin": "tool_result" }));
                i
            }
        };
        if let Some(Value::Object(m)) = self.steps.get_mut(idx) {
            let obs = m
                .entry("observation")
                .or_insert_with(|| json!({ "results": [] }));
            if let Some(Value::Array(a)) = obs.get_mut("results") {
                a.push(result);
            }
        }
    }

    /// Totals over every step's metrics (each step counted once).
    pub fn totals(&self) -> (u64, u64, u64) {
        let mut t = (0, 0, 0);
        for s in &self.steps {
            let m = &s["metrics"];
            t.0 += m["prompt_tokens"].as_u64().unwrap_or(0);
            t.1 += m["completion_tokens"].as_u64().unwrap_or(0);
            t.2 += m["cached_tokens"].as_u64().unwrap_or(0);
        }
        t
    }

    /// Drop agent steps left completely empty (no text, calls or reasoning).
    pub fn finish(mut self) -> Vec<Value> {
        self.steps.retain(|s| {
            s["source"] != "agent"
                || !s["message"].as_str().unwrap_or("").is_empty()
                || s.get("tool_calls").is_some()
                || s.get("reasoning_content").is_some()
        });
        for (i, s) in self.steps.iter_mut().enumerate() {
            s["step_id"] = json!(i + 1);
        }
        self.steps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calls_and_results_meet_and_ids_renumber() {
        let mut b = Builder::default();
        b.add("user", "hi", None);
        let empty = b.add("agent", "", None);
        let a = b.add("agent", "looking", None);
        b.call(a, "c1", "Bash", json!({"command": "ls"}));
        b.observe("c1", "file.txt");
        b.observe("unknown", "orphan");
        let _ = empty;
        let steps = b.finish();
        assert_eq!(steps.len(), 3, "the empty agent step is dropped");
        assert_eq!(steps[1]["tool_calls"][0]["function_name"], "Bash");
        assert_eq!(steps[1]["observation"]["results"][0]["content"], "file.txt");
        assert_eq!(steps[2]["source"], "system");
        let ids: Vec<u64> = steps
            .iter()
            .map(|s| s["step_id"].as_u64().unwrap())
            .collect();
        assert_eq!(ids, vec![1, 2, 3]);
    }

    #[test]
    fn clip_respects_char_boundaries() {
        let s = "é".repeat(MAX_OBSERVATION);
        let (c, extra) = clip(&s);
        assert!(c.len() <= MAX_OBSERVATION);
        assert!(extra.is_some());
    }
}
