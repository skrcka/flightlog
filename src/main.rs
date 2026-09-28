//! flightlog — record a coding-agent session as a portable, redacted bundle
//! (ATIF trajectory + native session for resume) and bring it back.

mod atif;
mod bundle;
mod redact;
mod restore;
mod skills;
mod sources;
mod targets;

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use serde_json::{json, Value};

use sources::Tool;

#[derive(Parser)]
#[command(name = "flightlog", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List agent sessions recorded for a directory (newest first).
    List {
        #[arg(long, value_enum)]
        tool: Option<Tool>,
        /// Directory the sessions ran in (default: current).
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// Export a session to a redacted .flightlog.zip bundle.
    Export {
        #[arg(long, value_enum)]
        tool: Option<Tool>,
        /// Session id (default: the newest session for --cwd).
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// JSON summary: {goal, state, decisions, open_questions, next_steps, files_touched}.
        #[arg(long)]
        summary: Option<PathBuf>,
        /// Summary goal (instead of --summary).
        #[arg(long)]
        goal: Option<String>,
        /// Summary state (instead of --summary).
        #[arg(long)]
        state: Option<String>,
        /// Output file (default: <session>.flightlog.zip).
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Leave out the tool's own session files (readable, not resumable).
        #[arg(long)]
        no_native: bool,
        /// Record that the redaction report was reviewed.
        #[arg(long)]
        reviewed: bool,
    },
    /// Show a bundle's summary, source, stats and redaction report.
    Inspect { file: PathBuf },
    /// Check a bundle against the format (exit 1 with every problem).
    Validate { file: PathBuf },
    /// Extract a bundle's files into a directory.
    Extract {
        file: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Upload a bundle: to an Inside task, or PUT to any presigned URL.
    Push {
        file: PathBuf,
        /// Inside task key or id (uses INSIDE_URL, INSIDE_MCP_TOKEN).
        #[arg(long, conflicts_with = "url")]
        task: Option<String>,
        /// Presigned upload URL.
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        inside_url: Option<String>,
    },
    /// Download a task's bundle from Inside (newest unless --context).
    Pull {
        task: String,
        #[arg(long)]
        context: Option<String>,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long)]
        inside_url: Option<String>,
    },
    /// Teach your agents to use flightlog: install its skills.
    Skills {
        #[command(subcommand)]
        cmd: SkillsCmd,
    },
    /// Put a bundle's native session back so its tool can resume it.
    Restore {
        file: PathBuf,
        /// Directory to resume in (default: current; must hold the same repository).
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// Overwrite existing session files.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum SkillsCmd {
    /// Install the flightlog-export and flightlog-import skills into
    /// ~/.agents/skills and each agent's skill folder (default: every agent
    /// found on this machine).
    Install {
        /// Only these agents.
        #[arg(long, value_enum, value_delimiter = ',')]
        agent: Vec<skills::Agent>,
        /// Install into this directory instead.
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Print a skill's SKILL.md (default: flightlog-export).
    Show { name: Option<String> },
}

fn cwd_of(p: Option<PathBuf>) -> Result<String> {
    let p = match p {
        Some(p) => p,
        None => std::env::current_dir()?,
    };
    Ok(sources::plain_path(
        &std::fs::canonicalize(&p).unwrap_or(p).to_string_lossy(),
    ))
}

fn default_summary(c: &sources::Converted) -> Value {
    let first_user = c
        .steps
        .iter()
        .find(|s| s["source"] == "user")
        .and_then(|s| s["message"].as_str())
        .unwrap_or("")
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .take(200)
        .collect::<String>();
    let goal = c
        .meta
        .title
        .clone()
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| {
            if first_user.is_empty() {
                format!("{} session {}", c.tool.id(), c.session_id)
            } else {
                first_user
            }
        });
    json!({
        "goal": goal,
        "state": format!("Exported by flightlog on {} ({} steps); see the conversation for detail.",
                         chrono::Utc::now().format("%Y-%m-%d"), c.steps.len()),
    })
}

fn print_findings(findings: &[redact::Finding]) {
    if findings.is_empty() {
        println!("redaction: nothing found");
        return;
    }
    println!("redaction (review before sharing):");
    for f in findings {
        let files: Vec<&str> = f.files.iter().map(String::as_str).collect();
        let shown = if files.len() > 3 {
            format!("{} and {} more", files[..3].join(", "), files.len() - 3)
        } else {
            files.join(", ")
        };
        println!("  {:<28} ×{:<4} {}", f.placeholder, f.count, shown);
    }
}

fn human(bytes: usize) -> String {
    if bytes >= 1 << 20 {
        format!("{:.1} MB", bytes as f64 / (1 << 20) as f64)
    } else {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    }
}

fn run() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::List { tool, cwd } => {
            let cwd = cwd_of(cwd)?;
            for t in tool.map(|t| vec![t]).unwrap_or_else(|| Tool::ALL.to_vec()) {
                for s in sources::list(t, &cwd).unwrap_or_default() {
                    let when = s
                        .modified
                        .map(|m| {
                            chrono::DateTime::<chrono::Local>::from(m)
                                .format("%Y-%m-%d %H:%M")
                                .to_string()
                        })
                        .unwrap_or_default();
                    println!(
                        "{:<12} {:<40} {}  {}",
                        t.id(),
                        s.id,
                        when,
                        s.title.unwrap_or_default()
                    );
                }
            }
        }
        Cmd::Export {
            tool,
            session,
            cwd,
            summary,
            goal,
            state,
            output,
            no_native,
            reviewed,
        } => {
            let cwd = cwd_of(cwd)?;
            let s = sources::pick(tool, session.as_deref(), &cwd)?;
            let c = sources::convert(&s, &cwd)?;
            let mut summary_v = match summary {
                Some(p) => serde_json::from_slice(
                    &std::fs::read(&p).with_context(|| format!("read {}", p.display()))?,
                )
                .context("summary must be JSON")?,
                None => default_summary(&c),
            };
            if let Some(g) = goal {
                summary_v["goal"] = json!(g);
            }
            if let Some(st) = state {
                summary_v["state"] = json!(st);
            }
            let built = bundle::build(
                &c,
                bundle::Build {
                    summary: summary_v,
                    include_native: !no_native,
                    reviewed,
                    cwd: &cwd,
                },
            )?;
            let out =
                output.unwrap_or_else(|| PathBuf::from(format!("{}.flightlog.zip", c.session_id)));
            std::fs::write(&out, &built.bytes)?;
            println!(
                "{} session {}: {} steps, {} native files, {}",
                c.tool.id(),
                c.session_id,
                c.steps.len(),
                if no_native { 0 } else { c.native.len() },
                human(built.bytes.len())
            );
            print_findings(&built.findings);
            if let Err(p) = bundle::open(&built.bytes) {
                bail!("the bundle does not validate:\n  - {}", p.join("\n  - "));
            }
            println!("wrote {}", out.display());
        }
        Cmd::Inspect { file } => {
            let bytes = std::fs::read(&file)?;
            let m = match bundle::open(&bytes) {
                Ok(b) => {
                    let steps = b.trajectory["steps"].as_array().map_or(0, Vec::len);
                    println!("valid bundle ({}, {steps} steps)", human(bytes.len()));
                    b.manifest
                }
                Err(p) => {
                    println!("INVALID:\n  - {}", p.join("\n  - "));
                    return Ok(());
                }
            };
            let src = &m["source"];
            println!(
                "source:  {} session {} {}",
                src["tool"].as_str().unwrap_or("?"),
                src["session_id"].as_str().unwrap_or("?"),
                src["title"]
                    .as_str()
                    .map(|t| format!("— {t}"))
                    .unwrap_or_default()
            );
            if let Some(cwd) = src["cwd"].as_str() {
                println!("cwd:     {cwd}");
            }
            let s = &m["summary"];
            println!("goal:    {}", s["goal"].as_str().unwrap_or(""));
            println!("state:   {}", s["state"].as_str().unwrap_or(""));
            for k in ["decisions", "open_questions", "next_steps", "files_touched"] {
                if let Some(a) = s[k].as_array().filter(|a| !a.is_empty()) {
                    println!("{k}:");
                    for x in a {
                        println!("  - {}", x.as_str().unwrap_or(&x.to_string()));
                    }
                }
            }
            println!("stats:   {}", m["stats"]);
            println!(
                "resume:  {}",
                m["native"]["resume_command"]
                    .as_str()
                    .unwrap_or("not resumable (no native session)")
            );
            let findings = m["redaction"]["findings"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            println!("redacted: {} placeholders", findings.len());
        }
        Cmd::Validate { file } => {
            bundle::read(&file)?;
            println!("valid");
        }
        Cmd::Extract { file, output } => {
            let b = bundle::read(&file)?;
            let dir = output.unwrap_or_else(|| {
                PathBuf::from(file.file_stem().unwrap_or_default()).with_extension("")
            });
            bundle::extract(&b, &dir)?;
            println!(
                "extracted to {} (conversation: trajectory.json)",
                dir.display()
            );
        }
        Cmd::Push {
            file,
            task,
            url,
            inside_url,
        } => {
            let bytes = std::fs::read(&file)?;
            if let Err(p) = bundle::open(&bytes) {
                bail!("not pushing an invalid bundle:\n  - {}", p.join("\n  - "));
            }
            let name = file
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            match (task, url) {
                (Some(t), _) => {
                    let rec =
                        targets::inside::Inside::from_env(inside_url)?.push(&t, &name, &bytes)?;
                    println!(
                        "attached to {t} as context {}",
                        rec["id"].as_str().unwrap_or("?")
                    );
                }
                (None, Some(u)) => {
                    targets::put(&u, &bytes)?;
                    println!("uploaded");
                }
                _ => bail!("pass --task (Inside) or --url (presigned PUT)"),
            }
        }
        Cmd::Pull {
            task,
            context,
            output,
            inside_url,
        } => {
            let inside = targets::inside::Inside::from_env(inside_url)?;
            let items = inside.list(&task)?;
            if items.is_empty() {
                bail!("{task} has no bundles");
            }
            for it in &items {
                println!(
                    "{}  {}  {:<12} {}",
                    it["id"].as_str().unwrap_or(""),
                    it["createdAt"]
                        .as_str()
                        .unwrap_or("")
                        .get(..16)
                        .unwrap_or(""),
                    it["source"]["tool"].as_str().unwrap_or(""),
                    it["summary"]["goal"]
                        .as_str()
                        .unwrap_or("")
                        .chars()
                        .take(80)
                        .collect::<String>()
                );
            }
            let chosen = match &context {
                Some(id) => items
                    .iter()
                    .find(|i| i["id"] == id.as_str())
                    .context("no such context on this task")?,
                None => &items[0],
            };
            let id = chosen["id"].as_str().unwrap_or("");
            let bytes = inside.download(id)?;
            let out = output.unwrap_or_else(|| {
                PathBuf::from(
                    chosen["filename"]
                        .as_str()
                        .unwrap_or("context.flightlog.zip"),
                )
            });
            std::fs::write(&out, &bytes)?;
            println!("downloaded {id} to {}", out.display());
        }
        Cmd::Skills {
            cmd: SkillsCmd::Install { agent, dir },
        } => {
            let dirs = match dir {
                Some(d) => vec![d],
                None => skills::targets(&agent),
            };
            for d in &dirs {
                skills::install_into(d)?;
                println!(
                    "installed flightlog-export, flightlog-import in {}",
                    d.display()
                );
            }
            println!("restart your agent so it loads the skills");
        }
        Cmd::Skills {
            cmd: SkillsCmd::Show { name },
        } => {
            let name = name.unwrap_or_else(|| "flightlog-export".into());
            let (_, body) = skills::SKILLS
                .iter()
                .find(|(n, _)| *n == name)
                .with_context(|| {
                    format!("no skill {name}; there are flightlog-export and flightlog-import")
                })?;
            print!("{body}");
        }
        Cmd::Restore { file, cwd, force } => {
            let b = bundle::read(&file)?;
            let cwd = cwd_of(cwd)?;
            let work =
                Path::new(".flightlog").join(b.manifest["bundle_id"].as_str().unwrap_or("bundle"));
            let r = restore::restore(&b, &cwd, &work, force)?;
            for p in &r.written {
                println!("restored {}", p.display());
            }
            if let Some(orig) = b.manifest["source"]["cwd"].as_str().filter(|o| *o != cwd) {
                println!("note: the session ran in {orig}; paths inside it still point there");
            }
            println!("now run: {}", r.next);
        }
    }
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("flightlog: {e:#}");
        std::process::exit(1);
    }
}
