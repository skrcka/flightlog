//! Where each agent keeps its sessions, and how to read one into ATIF.

pub mod claude;
pub mod codex;
pub mod copilot;
pub mod copilot_vscode;
pub mod cursor;
pub mod gemini;
pub mod opencode;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{bail, Result};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Tool {
    /// Claude Code
    Claude,
    /// OpenAI Codex CLI
    Codex,
    /// opencode
    Opencode,
    /// Gemini CLI
    Gemini,
    /// Cursor CLI (cursor-agent) and the Cursor editor
    Cursor,
    /// GitHub Copilot chat in VS Code and compatible editors
    CopilotVscode,
    /// GitHub Copilot CLI
    Copilot,
}

impl Tool {
    pub const ALL: [Tool; 7] = [
        Tool::Claude,
        Tool::Codex,
        Tool::Opencode,
        Tool::Gemini,
        Tool::Cursor,
        Tool::CopilotVscode,
        Tool::Copilot,
    ];

    /// `source.tool` in the manifest (spec §2.1).
    pub fn id(self) -> &'static str {
        match self {
            Tool::Claude => "claude_code",
            Tool::Codex => "codex",
            Tool::Opencode => "opencode",
            Tool::Gemini => "gemini_cli",
            Tool::Cursor => "cursor",
            Tool::CopilotVscode => "copilot_vscode",
            Tool::Copilot => "copilot_cli",
        }
    }
}

/// A session found on this machine.
#[derive(Debug, Clone)]
pub struct SessionRef {
    pub tool: Tool,
    pub id: String,
    /// The session file (Claude Code, Codex); opencode keeps a database.
    pub path: Option<PathBuf>,
    pub modified: Option<SystemTime>,
    pub title: Option<String>,
}

/// A file of the agent's own session, carried for exact resume (spec §5).
#[derive(Debug, Clone)]
pub struct NativeFile {
    pub path: String,
    pub bytes: Vec<u8>,
    /// Where it goes back (`~`, `{cwd_slug}` placeholders); `None` when the
    /// tool imports it by command instead.
    pub restore_to: Option<String>,
}

#[derive(Debug, Default, Clone)]
pub struct Meta {
    pub models: BTreeSet<String>,
    pub cwd: Option<String>,
    pub title: Option<String>,
    pub started: Option<String>,
    pub ended: Option<String>,
    pub compacted: bool,
    pub version: Option<String>,
}

impl Meta {
    pub fn seen(&mut self, ts: Option<&str>) {
        if let Some(ts) = ts {
            if self.started.is_none() {
                self.started = Some(ts.to_string());
            }
            self.ended = Some(ts.to_string());
        }
    }
}

/// A session converted to ATIF, with its native files.
#[derive(Debug)]
pub struct Converted {
    pub tool: Tool,
    pub session_id: String,
    pub agent_name: &'static str,
    pub steps: Vec<Value>,
    pub totals: (u64, u64, u64),
    pub meta: Meta,
    pub native: Vec<NativeFile>,
    pub layout: &'static str,
    pub resume: String,
}

/// Claude Code's project folder name for a working directory.
pub fn cwd_slug(path: &str) -> String {
    path.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// A path as the agents record it: no Windows `\\?\` verbatim prefix.
pub fn plain_path(p: &str) -> String {
    match p.strip_prefix(r"\\?\") {
        Some(rest) if rest.starts_with("UNC\\") => format!(r"\\{}", &rest[4..]),
        Some(rest) => rest.to_string(),
        None => p.to_string(),
    }
}

/// Whether two working directories are the same folder: separators, a
/// trailing slash and (on Windows and macOS) letter case don't matter.
pub fn same_dir(a: &str, b: &str) -> bool {
    fn key(p: &str) -> String {
        let p = plain_path(p).replace('\\', "/");
        let p = p.trim_end_matches('/');
        if cfg!(any(windows, target_os = "macos")) {
            p.to_lowercase()
        } else {
            p.to_string()
        }
    }
    key(a) == key(b)
}

pub fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

pub fn modified(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// Sessions of `tool` for `cwd`, newest first.
pub fn list(tool: Tool, cwd: &str) -> Result<Vec<SessionRef>> {
    match tool {
        Tool::Claude => claude::list(cwd),
        Tool::Codex => codex::list(cwd),
        Tool::Opencode => opencode::list(cwd),
        Tool::Gemini => gemini::list(cwd),
        Tool::Cursor => cursor::list(cwd),
        Tool::CopilotVscode => copilot_vscode::list(cwd),
        Tool::Copilot => copilot::list(cwd),
    }
}

/// The session to export: the one named, else the newest for `cwd` of the
/// given tool, else of any tool (newest wins).
pub fn pick(tool: Option<Tool>, session: Option<&str>, cwd: &str) -> Result<SessionRef> {
    if !crate::bypass::get().skip_path_checks
        && session.is_some_and(|id| !crate::safe_fs::identifier(id))
    {
        bail!("invalid session id");
    }
    let tools: Vec<Tool> = tool.map(|t| vec![t]).unwrap_or_else(|| Tool::ALL.to_vec());
    let mut found: Vec<SessionRef> = Vec::new();
    for t in tools {
        let all = match session {
            Some(id) => match t {
                Tool::Claude => claude::by_id(id).into_iter().collect(),
                Tool::Codex => codex::by_id(id).into_iter().collect(),
                Tool::Gemini => gemini::by_id(id).into_iter().collect(),
                Tool::Cursor => cursor::by_id(id, cwd).into_iter().collect(),
                Tool::CopilotVscode => copilot_vscode::by_id(id).into_iter().collect(),
                Tool::Copilot => copilot::by_id(id).into_iter().collect(),
                Tool::Opencode => opencode::by_id(id, cwd)?.into_iter().collect(),
            },
            None => list(t, cwd)?,
        };
        found.extend(all.into_iter().take(1));
    }
    found.sort_by_key(|s| std::cmp::Reverse(s.modified));
    match found.into_iter().next() {
        Some(s) => Ok(s),
        None => bail!(
            "no session found for {cwd}{}; try --tool and --session",
            session.map(|s| format!(" with id {s}")).unwrap_or_default()
        ),
    }
}

pub fn convert(s: &SessionRef, cwd: &str) -> Result<Converted> {
    match s.tool {
        Tool::Claude => claude::convert(s),
        Tool::Codex => codex::convert(s),
        Tool::Opencode => opencode::convert(s),
        Tool::Gemini => gemini::convert(s),
        Tool::Cursor => cursor::convert(s, cwd),
        Tool::CopilotVscode => copilot_vscode::convert(s),
        Tool::Copilot => copilot::convert(s),
    }
}

/// Plain text of a string or a list of `{type: text}` blocks.
pub fn blocks_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|b| match b {
                Value::String(s) => Some(s.clone()),
                Value::Object(_) if b["type"] == "text" => b["text"].as_str().map(str::to_string),
                Value::Object(_) if b["type"] == "image" => Some("[image]".into()),
                _ => None,
            })
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Every file under `dir`, recursively.
pub fn walk(dir: &Path) -> Vec<PathBuf> {
    let options = crate::bypass::get();
    let mut out = Vec::new();
    let mut pending = vec![(dir.to_path_buf(), 0)];
    let mut visited = std::collections::HashSet::new();
    while let Some((dir, depth)) = pending.pop() {
        if !options.skip_size_checks && (depth > 16 || out.len() >= 10_000) {
            continue;
        }
        let meta = if options.skip_path_checks {
            std::fs::metadata(&dir)
        } else {
            std::fs::symlink_metadata(&dir)
        };
        if !meta.is_ok_and(|m| m.is_dir()) {
            continue;
        }
        // Avoid rewalking the same directory when following explicit symlink overrides.
        if options.skip_path_checks
            && !visited.insert(std::fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone()))
        {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            if !options.skip_size_checks && out.len() >= 10_000 {
                break;
            }
            let kind = if options.skip_path_checks {
                std::fs::metadata(e.path()).map(|m| m.file_type())
            } else {
                e.file_type()
            };
            let Ok(kind) = kind else {
                continue;
            };
            if kind.is_dir() {
                pending.push((e.path(), depth + 1));
            } else if kind.is_file() {
                out.push(e.path());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_paths_compare_as_agents_record_them() {
        assert_eq!(plain_path(r"\\?\C:\Users\me\proj"), r"C:\Users\me\proj");
        assert_eq!(plain_path(r"\\?\UNC\srv\share\p"), r"\\srv\share\p");
        assert_eq!(plain_path("/home/me/p"), "/home/me/p");
        assert!(same_dir(r"\\?\C:\work\proj", r"C:\work\proj\"));
        assert!(same_dir(r"C:\work\proj", "C:/work/proj"));
        assert_eq!(cwd_slug(r"C:\Users\me\proj"), "C--Users-me-proj");
    }
}
