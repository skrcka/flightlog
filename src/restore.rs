//! Put a bundle's native session back where its tool expects it (SPEC.md §5).

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::bundle::Bundle;
use crate::sources::{cwd_slug, home};

pub struct Restored {
    pub written: Vec<PathBuf>,
    /// What the user runs next.
    pub next: String,
}

fn expand(template: &str, cwd: &str) -> PathBuf {
    let t = template.replace("{cwd_slug}", &cwd_slug(cwd));
    match t.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None => PathBuf::from(t),
    }
}

/// Restore into the current machine. Existing files are never overwritten
/// unless `force`.
pub fn restore(b: &Bundle, cwd: &str, work_dir: &Path, force: bool) -> Result<Restored> {
    let native = &b.manifest["native"];
    if native.is_null() {
        bail!("this bundle carries no native session (exported with --no-native?)");
    }
    let layout = native["layout"].as_str().unwrap_or("");
    let mut written = Vec::new();
    if layout == "opencode/export-v1" {
        let (name, bytes) = b
            .files
            .iter()
            .find(|(n, _)| n.starts_with("native/"))
            .context("opencode bundle without its export file")?;
        std::fs::create_dir_all(work_dir)?;
        let dest = work_dir.join(Path::new(name).file_name().unwrap_or_default());
        std::fs::write(&dest, bytes)?;
        written.push(dest.clone());
        return Ok(Restored {
            written,
            next: format!("opencode import {}", dest.display()),
        });
    }
    let entries = native["entries"].as_array().cloned().unwrap_or_default();
    // Check everything first, so a refusal leaves nothing half-restored.
    let mut plan = Vec::new();
    for e in &entries {
        let src = e["path"].as_str().context("native entry without path")?;
        let to = e["restore_to"]
            .as_str()
            .context("native entry without restore_to")?;
        let bytes = b
            .files
            .get(src)
            .with_context(|| format!("native file missing from bundle: {src}"))?;
        let dest = expand(to, cwd);
        if dest.exists() && !force {
            bail!(
                "{} already exists; it is probably the original session. Use --force to overwrite.",
                dest.display()
            );
        }
        plan.push((dest, bytes));
    }
    for (dest, bytes) in plan {
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&dest, bytes)?;
        written.push(dest);
    }
    Ok(Restored {
        written,
        next: native["resume_command"].as_str().unwrap_or("").to_string(),
    })
}
