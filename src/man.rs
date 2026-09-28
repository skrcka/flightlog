//! Man pages from the clap definitions: `flightlog.1` with the examples,
//! agent skills, files and environment sections, and `flightlog-<command>.1`
//! per command (`flightlog-skills-install.1` for nested ones).

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::Command;

const EXTRA: &str = r#".SH EXAMPLES
Export the newest session of the current directory with a summary:
.PP
.RS 4
.nf
flightlog export \-\-goal 'Migrate auth to OIDC' \-\-state 'Login works; logout TODO'
.fi
.RE
.PP
See what a bundle holds, then resume it:
.PP
.RS 4
.nf
flightlog inspect df9d8089.flightlog.zip
flightlog restore df9d8089.flightlog.zip
.fi
.RE
.PP
Hand a bundle to a service that gave you upload and download links:
.PP
.RS 4
.nf
flightlog push df9d8089.flightlog.zip \-\-url '<presigned PUT URL>'
flightlog pull '<signed URL>' \-o ctx.flightlog.zip
.fi
.RE
.SH AGENT SKILLS
.B flightlog skills install
writes two skills,
.B flightlog\-export
and
.BR flightlog\-import ,
that teach Claude Code, Codex, opencode, Copilot, Cursor and Gemini CLI to
save their own session and to pick one up again. They are also available as a
plugin from the repository github.com/skrcka/flightlog.
.SH FILES
Sessions are read from:
.TP
.I ~/.claude/projects/<cwd>/<id>.jsonl
Claude Code (the directory name is the working directory with every
non\-alphanumeric character replaced by \-).
.TP
.I ~/.codex/sessions/YYYY/MM/DD/rollout\-*.jsonl
Codex.
.TP
.I ~/.gemini/tmp/<project>/chats/session\-*.jsonl
Gemini CLI.
.TP
.I ~/.cursor/chats/<md5 of cwd>/<id>/store.db
Cursor CLI; editor chats come from Cursor's
.I state.vscdb
and are exported read\-only.
.PP
opencode sessions are read through
.BR "opencode export" .
.SH ENVIRONMENT
.TP
.B CLAUDE_CONFIG_DIR
Claude Code's configuration directory (default ~/.claude).
.TP
.B CODEX_HOME
Codex's home (default ~/.codex).
.TP
.B GEMINI_CLI_HOME
The directory holding .gemini (default: home).
.TP
.B CURSOR_CONFIG_DIR
The Cursor CLI's directory (default ~/.cursor).
.SH SEE ALSO
The bundle format: https://github.com/skrcka/flightlog/blob/main/SPEC.md
.br
Home page: https://flightlog.sh
"#;

/// The date of the newest CHANGELOG entry (`## [x.y.z] - YYYY-MM-DD`), so
/// the pages only change with a release.
fn release_date() -> &'static str {
    include_str!("../CHANGELOG.md")
        .lines()
        .find_map(|l| l.strip_prefix("## [")?.split(" - ").nth(1))
        .unwrap_or("")
}

fn page_for(cmd: Command) -> clap_mangen::Man {
    clap_mangen::Man::new(cmd)
        .date(release_date())
        .source(concat!("flightlog ", env!("CARGO_PKG_VERSION")))
        .manual("flightlog manual")
}

fn pages(cmd: &Command, prefix: &str, out: &mut Vec<(String, Command)>) {
    for sub in cmd.get_subcommands().filter(|s| !s.is_hide_set()) {
        let page = format!("{prefix}-{}", sub.get_name());
        let bin = page.replace('-', " ");
        let c = sub.clone().display_name(page.clone()).bin_name(bin);
        pages(&c, &page, out);
        out.push((page, c));
    }
}

/// Write every page into `dir`; returns the files written.
pub fn write_all(cli: &Command, dir: &Path) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dir)?;
    let mut written = Vec::new();

    let mut main = Vec::new();
    let man = page_for(cli.clone());
    man.render_title(&mut main)?;
    man.render_name_section(&mut main)?;
    man.render_synopsis_section(&mut main)?;
    man.render_description_section(&mut main)?;
    man.render_options_section(&mut main)?;
    man.render_subcommands_section(&mut main)?;
    main.write_all(EXTRA.as_bytes())?;
    man.render_version_section(&mut main)?;
    let f = dir.join("flightlog.1");
    std::fs::write(&f, normalize(&main)?)?;
    written.push(f);

    let mut subs = Vec::new();
    pages(cli, "flightlog", &mut subs);
    for (name, c) in subs {
        let mut buf = Vec::new();
        page_for(c).render(&mut buf)?;
        buf.write_all(b".SH SEE ALSO\n.BR flightlog (1)\n")?;
        let f = dir.join(format!("{name}.1"));
        std::fs::write(&f, normalize(&buf)?)?;
        written.push(f);
    }
    written.sort();
    Ok(written)
}

fn normalize(bytes: &[u8]) -> Result<String> {
    Ok(std::str::from_utf8(bytes)?
        .lines()
        .map(|line| format!("{}\n", line.trim_end()))
        .collect())
}
