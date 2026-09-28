//! flightlog — record a coding-agent session as a portable, redacted bundle
//! (ATIF trajectory + native session for resume) and bring it back.

mod atif;
mod bundle;
mod bypass;
mod man;
mod redact;
mod restore;
mod safe_fs;
mod skills;
mod sources;
mod targets;

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use serde_json::{json, Value};

use sources::Tool;

const AFTER_HELP: &str = "\
Examples:
  flightlog list                                  sessions recorded for this directory
  flightlog export --goal 'Fix login' --state 'Done; needs review'
  flightlog inspect <id>.flightlog.zip            summary, stats, what was redacted
  flightlog restore <id>.flightlog.zip            put it back, print the resume command

Teach your agent to do this itself:
  flightlog skills install

Format: https://github.com/skrcka/flightlog/blob/main/SPEC.md
Docs:   https://flightlog.sh  ·  man flightlog";

#[derive(Parser)]
#[command(
    name = "flightlog",
    version,
    about = "Save coding-agent sessions as portable, redacted bundles, and bring them back",
    long_about = "Save coding-agent sessions as portable, redacted bundles, and bring them back.\n\n\
        flightlog reads a session of Claude Code, Codex, opencode, Gemini CLI or Cursor \
        and writes one .flightlog.zip: the conversation as an ATIF trajectory, a summary \
        for whoever picks it up, a redaction report, and the tool's own session files so \
        the session can be resumed. Secrets are replaced with [REDACTED:<kind>:<n>] \
        placeholders before anything is written by default.",
    after_help = AFTER_HELP,
    disable_help_subcommand = true
)]
struct Cli {
    #[command(flatten)]
    overrides: bypass::Overrides,
    /// Accept explicitly unredacted bundles for private migration
    #[arg(long, global = true)]
    allow_unredacted: bool,
    /// Disable redaction and every bypassable safety check; overwrite outputs
    #[arg(long, global = true)]
    yolo: bool,
    /// File of private literal names/domains to redact (one per line; kept local)
    #[arg(long, global = true, value_name = "FILE")]
    redact_file: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List sessions recorded for a directory, newest first
    #[command(display_order = 1)]
    List {
        /// Only this tool's sessions
        #[arg(long, value_enum, value_name = "TOOL")]
        tool: Option<Tool>,
        /// Directory the sessions ran in [default: current directory]
        #[arg(long, value_name = "DIR")]
        cwd: Option<PathBuf>,
    },
    /// Export a session to a redacted .flightlog.zip
    #[command(
        display_order = 2,
        long_about = "Export a session to a redacted .flightlog.zip.\n\n\
            Picks the newest session recorded for the directory (any tool) unless \
            --tool or --session say otherwise. Prints the redaction report, then the \
            file's size and SHA-256. Review the report before sharing the bundle.",
        after_help = "Examples:\n  \
            flightlog export --goal 'Migrate auth to OIDC' --state 'Login works; logout TODO'\n  \
            flightlog export --summary summary.json --reviewed -o handoff.flightlog.zip\n  \
            flightlog export --tool codex --session <id> --no-native"
    )]
    Export {
        /// Preserve sensitive content for private migration (bundle is not encrypted)
        #[arg(long)]
        no_redact: bool,
        /// Tool the session belongs to [default: whichever has the newest session]
        #[arg(long, value_enum, value_name = "TOOL")]
        tool: Option<Tool>,
        /// Session id [default: the newest session for the directory]
        #[arg(long, value_name = "ID")]
        session: Option<String>,
        /// Directory the session ran in [default: current directory]
        #[arg(long, value_name = "DIR")]
        cwd: Option<PathBuf>,
        /// Summary as JSON: {goal, state, decisions, open_questions, next_steps, files_touched}
        #[arg(long, value_name = "FILE", help_heading = "Summary")]
        summary: Option<PathBuf>,
        /// What the session set out to do (instead of --summary)
        #[arg(long, value_name = "TEXT", help_heading = "Summary")]
        goal: Option<String>,
        /// Where it stands now (instead of --summary)
        #[arg(long, value_name = "TEXT", help_heading = "Summary")]
        state: Option<String>,
        /// Output file [default: <session>.flightlog.zip]
        #[arg(short, long, value_name = "FILE")]
        output: Option<PathBuf>,
        /// Leave out the tool's own session files: readable, but not resumable
        #[arg(long)]
        no_native: bool,
        /// Record in the bundle that a person reviewed the redaction report
        #[arg(long)]
        reviewed: bool,
        /// Include working directory and Git metadata (may identify private projects)
        #[arg(long)]
        include_metadata: bool,
    },
    /// Show a bundle's summary, source, stats and redaction report
    #[command(display_order = 3)]
    Inspect {
        /// The .flightlog.zip
        file: PathBuf,
    },
    /// Check a bundle against the format; exits 1 listing every problem
    #[command(display_order = 4)]
    Validate {
        /// The .flightlog.zip
        file: PathBuf,
    },
    /// Unpack a bundle into a directory (the conversation is trajectory.json)
    #[command(display_order = 5)]
    Extract {
        /// The .flightlog.zip
        file: PathBuf,
        /// Target directory [default: the file name without .zip]
        #[arg(short, long, value_name = "DIR")]
        output: Option<PathBuf>,
    },
    /// Put a bundle's session back so its tool can resume it
    #[command(
        display_order = 6,
        long_about = "Put a bundle's session back so its tool can resume it.\n\n\
            Writes the tool's own session files where it looks for them (never \
            overwriting without --force, --overwrite or --yolo) and prints the command to run: \
            claude --resume, codex resume, opencode import, gemini --resume or \
            cursor-agent --resume. Paths inside a session are absolute, so resume in \
            a directory holding the same repository."
    )]
    Restore {
        /// The .flightlog.zip
        file: PathBuf,
        /// Directory to resume in [default: current directory]
        #[arg(long, value_name = "DIR")]
        cwd: Option<PathBuf>,
        /// Overwrite session files that already exist
        #[arg(long)]
        force: bool,
    },
    /// Upload a bundle to a presigned URL (validated first)
    #[command(display_order = 7)]
    Push {
        /// The .flightlog.zip
        file: PathBuf,
        /// Presigned upload URL (HTTP PUT)
        #[arg(long, value_name = "URL")]
        url: Option<String>,
        /// Read the signed URL from a private file, or - for stdin
        #[arg(long, conflicts_with = "url", required_unless_present = "url")]
        url_file: Option<PathBuf>,
    },
    /// Download a bundle from a URL and validate it
    #[command(display_order = 8)]
    Pull {
        /// Download URL, e.g. a signed link
        url: Option<String>,
        /// Read the signed URL from a private file, or - for stdin
        #[arg(long, conflicts_with = "url", required_unless_present = "url")]
        url_file: Option<PathBuf>,
        /// Where to save it
        #[arg(
            short,
            long,
            value_name = "FILE",
            default_value = "session.flightlog.zip"
        )]
        output: PathBuf,
    },
    /// Install the agent skills that teach agents to export and import sessions
    #[command(
        display_order = 9,
        long_about = "Install the agent skills that teach agents to export and import sessions.\n\n\
            Two skills, flightlog-export and flightlog-import, tell an agent how to save \
            its own session (with a summary it writes) and how to pick one up again. \
            They are built into this binary. Agents can also get them as a plugin: \
            claude|codex|copilot plugin marketplace add skrcka/flightlog, or \
            gemini extensions install https://github.com/skrcka/flightlog."
    )]
    Skills {
        #[command(subcommand)]
        cmd: SkillsCmd,
    },
    /// Write man pages (for packaging)
    #[command(hide = true)]
    Man {
        /// Directory for flightlog.1 and flightlog-<command>.1
        #[arg(long, value_name = "DIR", default_value = "man")]
        dir: PathBuf,
    },
}

#[derive(Subcommand)]
enum SkillsCmd {
    /// Install the skills for every agent found here, or the ones named
    #[command(
        long_about = "Install the skills for every agent found here, or the ones named.\n\n\
            Always writes ~/.agents/skills (read by opencode, Copilot, Cursor, Gemini CLI \
            and Codex), plus the skill folder of each agent whose config directory \
            exists. Restart the agent afterwards.",
        after_help = "Examples:\n  \
            flightlog skills install\n  \
            flightlog skills install --agent claude,codex\n  \
            flightlog skills install --dir .claude/skills      # this project only"
    )]
    Install {
        /// Only these agents (comma-separated)
        #[arg(long, value_enum, value_delimiter = ',', value_name = "AGENT")]
        agent: Vec<skills::Agent>,
        /// Install into this directory instead
        #[arg(long, value_name = "DIR")]
        dir: Option<PathBuf>,
    },
    /// Print a skill's SKILL.md
    Show {
        /// flightlog-export or flightlog-import
        #[arg(default_value = "flightlog-export")]
        name: String,
    },
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

fn safe_display_value(v: &Value) -> Value {
    match v {
        Value::String(s) => json!(safe_fs::text(s)),
        Value::Array(a) => Value::Array(a.iter().map(safe_display_value).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| (safe_fs::text(k), safe_display_value(v)))
                .collect(),
        ),
        _ => v.clone(),
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    bypass::init(cli.overrides, cli.yolo);
    if cli.yolo {
        eprintln!(
            "YOLO: redaction, validation, path and size protections disabled; overwrites enabled"
        );
    }
    let allow_unredacted = cli.allow_unredacted || cli.yolo;
    if let Some(path) = cli.redact_file {
        std::env::set_var("FLIGHTLOG_REDACT_FILE", path);
    }
    match cli.cmd {
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
                        safe_fs::text(&s.id),
                        when,
                        safe_fs::text(&s.title.unwrap_or_default())
                    );
                }
            }
        }
        Cmd::Export {
            no_redact,
            tool,
            session,
            cwd,
            summary,
            goal,
            state,
            output,
            no_native,
            reviewed,
            include_metadata,
        } => {
            let no_redact = no_redact || cli.yolo;
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
                    no_redact,
                    summary: summary_v,
                    include_native: !no_native,
                    reviewed,
                    include_metadata,
                    cwd: &cwd,
                },
            )?;
            let out =
                output.unwrap_or_else(|| PathBuf::from(format!("{}.flightlog.zip", c.session_id)));
            if !bypass::get().skip_path_checks && !safe_fs::identifier(&c.session_id) {
                bail!("unsafe session identifier");
            }
            if let Err(p) = bundle::open_with_policy(&built.bytes, no_redact) {
                bail!("export failed validation: {}", p.join("; "));
            }
            safe_fs::write(&out, &built.bytes, false)?;
            println!(
                "{} session {}: {} steps, {} native files, {}",
                c.tool.id(),
                c.session_id,
                c.steps.len(),
                if no_native { 0 } else { c.native.len() },
                human(built.bytes.len())
            );
            if no_redact {
                eprintln!("redaction: DISABLED — unencrypted bundle may contain credentials and private data");
            } else {
                print_findings(&built.findings);
            }
            println!(
                "wrote {} ({} bytes, sha256 {})",
                out.display(),
                built.bytes.len(),
                bundle::sha256_hex(&built.bytes)
            );
        }
        Cmd::Inspect { file } => {
            let bytes = safe_fs::read(&file, bundle::MAX_ARCHIVE_BYTES)?;
            let m = match bundle::open_with_policy(&bytes, allow_unredacted) {
                Ok(b) => {
                    let steps = b.trajectory["steps"].as_array().map_or(0, Vec::len);
                    println!("valid bundle ({}, {steps} steps)", human(bytes.len()));
                    b.manifest
                }
                Err(p) => {
                    bail!("invalid bundle: {}", p.join("; "));
                }
            };
            let m = safe_display_value(&m);
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
                "resume: {}",
                if m["native"].is_object() {
                    "use flightlog restore to derive a safe command"
                } else {
                    "not resumable"
                }
            );
            let findings = m["redaction"]["findings"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            if m["redaction"]["mode"] == "none" {
                println!("redaction: DISABLED (unredacted private migration bundle)");
            } else {
                println!("redacted: {} placeholders", findings.len());
            }
        }
        Cmd::Validate { file } => {
            bundle::read(&file, allow_unredacted)?;
            println!("valid");
        }
        Cmd::Extract { file, output } => {
            let b = bundle::read(&file, allow_unredacted)?;
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
            url,
            url_file,
        } => {
            let bytes = safe_fs::read(&file, bundle::MAX_ARCHIVE_BYTES)?;
            if let Err(p) = bundle::open_with_policy(&bytes, allow_unredacted) {
                bail!("not pushing an invalid bundle:\n  - {}", p.join("\n  - "));
            }
            targets::put(&targets::url_input(url, url_file)?, &bytes)?;
            println!("uploaded {} ({} bytes)", file.display(), bytes.len());
        }
        Cmd::Pull {
            url,
            output,
            url_file,
        } => {
            let bytes = targets::get(&targets::url_input(url, url_file)?)?;
            if let Err(p) = bundle::open_with_policy(&bytes, allow_unredacted) {
                bail!(
                    "the download is not a valid bundle:\n  - {}",
                    p.join("\n  - ")
                );
            }
            safe_fs::write(&output, &bytes, false)?;
            println!("downloaded to {}", output.display());
        }
        Cmd::Man { dir } => {
            for f in man::write_all(&Cli::command(), &dir)? {
                println!("{}", f.display());
            }
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
            let (_, body) = skills::SKILLS
                .iter()
                .find(|(n, _)| *n == name)
                .with_context(|| {
                    format!("no skill {name}; there are flightlog-export and flightlog-import")
                })?;
            print!("{body}");
        }
        Cmd::Restore { file, cwd, force } => {
            let b = bundle::read(&file, allow_unredacted)?;
            let cwd = cwd_of(cwd)?;
            let work =
                Path::new(".flightlog").join(b.manifest["bundle_id"].as_str().unwrap_or("bundle"));
            let r = restore::restore(&b, &cwd, &work, force)?;
            for p in &r.written {
                println!("restored {}", safe_fs::display(p));
            }
            if let Some(orig) = b.manifest["source"]["cwd"].as_str().filter(|o| *o != cwd) {
                println!(
                    "note: the session ran in {}; paths inside it still point there",
                    safe_fs::text(orig)
                );
            }
            println!("now run: {}", safe_fs::text(&r.next));
        }
    }
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("flightlog: {}", safe_fs::text(&format!("{e:#}")));
        std::process::exit(1);
    }
}
