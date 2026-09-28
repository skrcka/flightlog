//! The agent skills that teach Claude Code, Codex, opencode, Copilot, Cursor
//! and Gemini CLI how to use flightlog, embedded so `flightlog skills install` can place them.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::sources::home;

pub const SKILLS: &[(&str, &str)] = &[
    (
        "flightlog-export",
        include_str!("../skills/flightlog-export/SKILL.md"),
    ),
    (
        "flightlog-import",
        include_str!("../skills/flightlog-import/SKILL.md"),
    ),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Agent {
    Claude,
    Codex,
    Opencode,
    Copilot,
    Cursor,
    Gemini,
}

impl Agent {
    pub const ALL: [Agent; 6] = [
        Agent::Claude,
        Agent::Codex,
        Agent::Opencode,
        Agent::Copilot,
        Agent::Cursor,
        Agent::Gemini,
    ];

    /// The agent's config directory (present = the agent is installed).
    fn config_dir(self) -> PathBuf {
        match self {
            Agent::Claude => std::env::var_os("CLAUDE_CONFIG_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| home().join(".claude")),
            Agent::Codex => std::env::var_os("CODEX_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home().join(".codex")),
            Agent::Opencode => config_home().join("opencode"),
            Agent::Copilot => home().join(".copilot"),
            Agent::Cursor => std::env::var_os("CURSOR_CONFIG_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| home().join(".cursor")),
            Agent::Gemini => home().join(".gemini"),
        }
    }

    pub fn skills_dir(self) -> PathBuf {
        self.config_dir().join("skills")
    }
}

fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"))
}

/// The shared Agent Skills folder (read by opencode, Copilot, Cursor, Gemini
/// CLI and Codex).
pub fn shared_dir() -> PathBuf {
    home().join(".agents").join("skills")
}

/// Write every skill into `dir/<name>/SKILL.md`, replacing earlier versions
/// of flightlog's own skills only.
pub fn install_into(dir: &Path) -> Result<()> {
    for (name, body) in SKILLS {
        let d = dir.join(name);
        std::fs::create_dir_all(&d)?;
        std::fs::write(d.join("SKILL.md"), body)?;
    }
    Ok(())
}

/// Target folders: the named agents, or every agent found on this machine,
/// plus the shared folder.
pub fn targets(agents: &[Agent]) -> Vec<PathBuf> {
    let chosen: Vec<Agent> = if agents.is_empty() {
        Agent::ALL
            .into_iter()
            .filter(|a| a.config_dir().is_dir())
            .collect()
    } else {
        agents.to_vec()
    };
    let mut dirs = vec![shared_dir()];
    for a in chosen {
        let d = a.skills_dir();
        if !dirs.contains(&d) {
            dirs.push(d);
        }
    }
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skills_have_matching_frontmatter() {
        for (name, body) in SKILLS {
            assert!(body.starts_with("---\n"), "{name}");
            assert!(body.contains(&format!("\nname: {name}\n")), "{name}");
            assert!(body.contains("\ndescription: "), "{name}");
        }
    }

    #[test]
    fn install_writes_each_skill() {
        let dir = tempfile::tempdir().unwrap();
        install_into(dir.path()).unwrap();
        for (name, _) in SKILLS {
            assert!(dir.path().join(name).join("SKILL.md").is_file());
        }
    }
}
