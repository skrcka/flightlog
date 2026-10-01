//! Restore only known layouts into locally selected agent session roots.
use crate::{
    bundle::Bundle,
    safe_fs,
    sources::{self, cursor, gemini},
};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
pub struct Restored {
    pub written: Vec<PathBuf>,
    pub next: String,
}

pub fn restore(b: &Bundle, cwd: &str, work_dir: &Path, force: bool) -> Result<Restored> {
    let options = crate::bypass::get();
    let force = force || options.overwrite;
    let native = &b.manifest["native"];
    let id = b.manifest["source"]["session_id"]
        .as_str()
        .context("missing session id")?;
    if !options.skip_path_checks && !safe_fs::identifier(id) {
        bail!("invalid session id");
    }
    let layout = native["layout"]
        .as_str()
        .context("bundle has no native session; use flightlog restore FILE --to TOOL to convert its shared history")?;
    let tool = b.manifest["source"]["tool"].as_str().unwrap_or("");
    let files: Vec<_> = b
        .files
        .iter()
        .filter(|(n, _)| n.starts_with("native/"))
        .collect();
    if files.is_empty() {
        bail!("bundle has no native files; use flightlog restore FILE --to TOOL to convert its shared history");
    }
    let expected = match layout {
        cursor::CLI_LAYOUT => "cursor",
        "opencode/export-v1" => "opencode",
        "claude_code/projects-v1" => "claude_code",
        "codex/rollout-v1" => "codex",
        "gemini-cli/chats-v1" => "gemini_cli",
        sources::copilot_vscode::LAYOUT => "copilot_vscode",
        sources::copilot::LAYOUT => "copilot_cli",
        _ if options.skip_format_checks && options.skip_path_checks => tool,
        _ => bail!("unsupported native layout"),
    };
    if !options.skip_format_checks && (tool != expected || native["tool"] != expected) {
        bail!("native tool/layout mismatch");
    }
    if options.skip_path_checks
        && !matches!(
            layout,
            cursor::CLI_LAYOUT
                | "opencode/export-v1"
                | sources::copilot_vscode::LAYOUT
                | sources::copilot::LAYOUT
        )
    {
        return restore_templates(b, cwd, force, files.len());
    }
    if layout == cursor::CLI_LAYOUT {
        if !options.skip_format_checks
            && (files.len() != 1 || files[0].0 != &format!("native/{id}.cursor-store.json"))
        {
            bail!("unexpected Cursor native files");
        }
        let dir = cursor::restore_cli(files[0].1, id, cwd, force)?;
        return Ok(Restored {
            written: vec![dir.join("store.db"), dir.join("meta.json")],
            next: if safe_fs::identifier(id) {
                format!("cursor-agent --resume {id}")
            } else {
                "resume the restored session in Cursor".into()
            },
        });
    }
    if layout == sources::copilot::LAYOUT {
        if files.len() != 1 || files[0].0 != &format!("native/{id}.jsonl") {
            bail!("unexpected Copilot native files");
        }
        let events = sources::copilot::parse(files[0].1)?;
        if events[0]["data"]["sessionId"] != id {
            bail!("Copilot session id mismatch");
        }
        let dest = sources::copilot::copilot_home()
            .join("session-state")
            .join(id)
            .join("events.jsonl");
        safe_fs::write(&dest, files[0].1, force)?;
        return Ok(Restored {
            written: vec![dest],
            next: format!("copilot --resume {id}"),
        });
    }
    if matches!(
        layout,
        "opencode/export-v1" | sources::copilot_vscode::LAYOUT
    ) {
        if !options.skip_format_checks
            && (files.len() != 1 || files[0].0 != &format!("native/{id}.json"))
        {
            bail!("unexpected native import files");
        }
        if !options.skip_path_checks {
            uuid::Uuid::parse_str(b.manifest["bundle_id"].as_str().unwrap_or(""))?;
        }
        let dest = work_dir.join(format!("{id}.json"));
        safe_fs::write(&dest, files[0].1, force)?;
        if layout == sources::copilot_vscode::LAYOUT {
            return Ok(Restored {
                next: format!(
                    "In VS Code, run Chat: Import Chat from the Command Palette and select {}",
                    safe_fs::display(&dest)
                ),
                written: vec![dest],
            });
        }
        #[cfg(not(windows))]
        let next = format!(
            "opencode import {}",
            safe_fs::quote(&dest.to_string_lossy())
        );
        #[cfg(windows)]
        let next = format!(
            "opencode import (file argument: {})",
            safe_fs::display(&dest)
        );
        return Ok(Restored {
            written: vec![dest],
            next,
        });
    }
    let entries = native["entries"]
        .as_array()
        .context("missing native entries")?;
    if entries.len() != files.len() {
        bail!("native entries must account for every native file");
    }
    let mut plan = Vec::new();
    let mut registration = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for e in entries {
        let name = e["path"].as_str().context("missing native path")?;
        if !seen.insert(name) {
            bail!("duplicate native path");
        }
        let rel = name
            .strip_prefix("native/")
            .context("native path outside native/")?;
        if !safe_fs::safe_relative(rel) {
            bail!("unsafe native path");
        }
        let bytes = b.files.get(name).context("missing native file")?;
        let template = e["restore_to"]
            .as_str()
            .context("missing destination template")?;
        let dest = match layout {
            "claude_code/projects-v1" => {
                if rel != format!("{id}.jsonl") && !rel.starts_with(&format!("{id}/")) {
                    bail!("native file does not belong to the session");
                }
                if template != format!("~/.claude/projects/{{cwd_slug}}/{rel}") {
                    bail!("unexpected destination template");
                }
                sources::claude::projects_dir()
                    .join(sources::cwd_slug(cwd))
                    .join(rel)
            }
            "codex/rollout-v1" => {
                let target = template
                    .strip_prefix("~/.codex/sessions/")
                    .context("unexpected destination template")?;
                let parts: Vec<_> = target.split('/').collect();
                if !safe_fs::safe_relative(target)
                    || parts.len() != 4
                    || parts[0].len() != 4
                    || parts[1].len() != 2
                    || parts[2].len() != 2
                    || !parts[..3]
                        .iter()
                        .all(|s| s.bytes().all(|c| c.is_ascii_digit()))
                    || parts[3] != rel
                    || !rel.starts_with("rollout-")
                    || !rel.ends_with(&format!("{id}.jsonl"))
                    || files.len() != 1
                {
                    bail!("invalid Codex session layout");
                }
                sources::codex::codex_home().join("sessions").join(target)
            }
            "gemini-cli/chats-v1" => {
                if rel.contains('/')
                    || !rel.starts_with("session-")
                    || !(rel.ends_with(".jsonl") || rel.ends_with(".json"))
                    || files.len() != 1
                    || template != format!("~/.gemini/tmp/{{gemini_project}}/chats/{rel}")
                {
                    bail!("invalid Gemini session layout");
                }
                // Resolve registration only after all untrusted structure is checked.
                let project = gemini::project_for_restore(cwd)?;
                if !safe_fs::identifier(&project.name) {
                    bail!("invalid Gemini project identifier");
                }
                registration = project.writes;
                gemini::gemini_home()
                    .join("tmp")
                    .join(project.name)
                    .join("chats")
                    .join(rel)
            }
            _ => unreachable!(),
        };
        safe_fs::check_destination(&dest, force)?;
        plan.push((dest, bytes));
    }
    let mut written = Vec::new();
    // Check every parent before writing session data or registration files.
    for (dest, _) in &plan {
        safe_fs::dir(dest.parent().context("missing destination parent")?)?;
    }
    for (dest, _, replace) in &registration {
        safe_fs::check_destination(dest, *replace)?;
        safe_fs::dir(dest.parent().context("missing registration parent")?)?;
    }
    for (dest, bytes) in plan {
        safe_fs::write(&dest, bytes, force)?;
        written.push(dest);
    }
    for (dest, bytes, replace) in registration {
        safe_fs::write(&dest, &bytes, replace)?;
    }
    let next = match tool {
        "claude_code" => format!("claude --resume {id}"),
        "codex" => format!("codex resume {id}"),
        _ => format!("gemini --resume {id}"),
    };
    Ok(Restored { written, next })
}

/// Explicitly trust destinations, while retaining format/overwrite checks unless
/// those were independently disabled. Never execute archive commands or SQL.
fn restore_templates(b: &Bundle, cwd: &str, force: bool, native_count: usize) -> Result<Restored> {
    let options = crate::bypass::get();
    let entries = b.manifest["native"]["entries"]
        .as_array()
        .filter(|entries| !entries.is_empty())
        .context("missing native entries")?;
    if !options.skip_format_checks && entries.len() != native_count {
        bail!("native entries must account for every native file");
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut plan = Vec::new();
    let mut registration = Vec::new();
    let mut gemini_project = None;
    for entry in entries {
        let name = entry["path"].as_str().context("missing native path")?;
        if !options.skip_format_checks && (!name.starts_with("native/") || !seen.insert(name)) {
            bail!("invalid or duplicate native path");
        }
        let bytes = b.files.get(name).context("missing native file")?;
        let mut target = entry["restore_to"]
            .as_str()
            .context("missing destination")?
            .replace("{cwd_slug}", &sources::cwd_slug(cwd));
        if target.contains("{gemini_project}") {
            if gemini_project.is_none() {
                let project = gemini::project_for_restore(cwd)?;
                registration = project.writes;
                gemini_project = Some(project.name);
            }
            target = target.replace("{gemini_project}", gemini_project.as_deref().unwrap());
        }
        let dest = if let Some(rel) = target.strip_prefix("~/.codex/") {
            sources::codex::codex_home().join(rel)
        } else if let Some(rel) = target.strip_prefix("~/.claude/projects/") {
            sources::claude::projects_dir().join(rel)
        } else if let Some(rel) = target.strip_prefix("~/.gemini/") {
            gemini::gemini_home().join(rel)
        } else if let Some(rel) = target.strip_prefix("~/") {
            sources::home().join(rel)
        } else {
            PathBuf::from(target)
        };
        safe_fs::check_destination(&dest, force)?;
        plan.push((dest, bytes));
    }
    for (dest, _, replace) in &registration {
        safe_fs::check_destination(dest, *replace)?;
    }
    let mut written = Vec::new();
    for (dest, bytes) in plan {
        safe_fs::write(&dest, bytes, force)?;
        written.push(dest);
    }
    for (dest, bytes, replace) in registration {
        safe_fs::write(&dest, &bytes, replace)?;
    }
    Ok(Restored {
        written,
        next: "resume the restored session in its original tool".into(),
    })
}
