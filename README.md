# flightlog

Record a coding-agent session — **Claude Code, Codex, opencode** — as a
portable, redacted bundle, and bring it back: read it anywhere, or resume it in
the tool it came from.

```sh
curl -fsSL https://flightlog.sh/install.sh | sh     # macOS, Linux
```

```powershell
irm https://flightlog.sh/install.ps1 | iex          # Windows
```

[flightlog.sh](https://flightlog.sh)

```sh
flightlog export                      # newest session in this directory → <id>.flightlog.zip
flightlog inspect <id>.flightlog.zip  # summary, stats, what was redacted
flightlog restore <id>.flightlog.zip  # put it back; prints `claude --resume …`
```

## Why

Agent sessions hold the real context of a piece of work: what was tried, what
was decided, what is left. They live in each tool's private files, in each
tool's own format, full of whatever secrets the agent saw. flightlog turns one
into a single file you can attach to a ticket, hand to a colleague or another
agent, and later resume:

- **Open format.** The conversation is [ATIF v1.8](https://github.com/harbor-framework/harbor/blob/main/rfcs/0001-trajectory-format.md)
  (the Agent Trajectory Interchange Format used by Harbor and others), wrapped
  in a small manifest: summary, checksums, redaction report. See [SPEC.md](SPEC.md).
- **Redacted by default.** API keys, tokens, private keys, URL and `.env`
  passwords become stable `[REDACTED:<kind>:<n>]` placeholders; you see the
  report before anything leaves your machine.
- **Resumable.** The tool's own session files ride along, so `restore` puts
  them back and `claude --resume`, `codex resume` or `opencode import` picks up
  exactly where it stopped.
- **One static binary.** No Python or Node needed.

## Commands

| Command | What it does |
|---|---|
| `flightlog list` | Sessions recorded for this directory, newest first |
| `flightlog export [--tool claude\|codex\|opencode] [--session ID]` | Build a bundle; `--summary summary.json` or `--goal/--state` add the summary, `--no-native` leaves out the resumable files |
| `flightlog inspect FILE` | Summary, source, stats, redaction report |
| `flightlog validate FILE` | Check against the spec (exit 1 with every problem) |
| `flightlog extract FILE [-o DIR]` | Unpack; the conversation is `trajectory.json` |
| `flightlog restore FILE` | Put the native session back and print the resume command (never overwrites without `--force`) |
| `flightlog push FILE --url URL` | Upload to any presigned PUT URL |
| `flightlog push FILE --task KEY` / `flightlog pull KEY` | Attach to / fetch from an [Inside](#inside) task |

### Summaries

`export` writes a basic summary from the session title. For a useful one, let
the agent write it before exporting:

```json
{ "goal": "what the session set out to do",
  "state": "where it stands: done, blocked, what is left",
  "decisions": ["…"], "open_questions": ["…"], "next_steps": ["…"],
  "files_touched": ["…"] }
```

```sh
flightlog export --summary summary.json --reviewed
```

### Where sessions are read from

`~` is `%USERPROFILE%` on Windows.

| Tool | Location |
|---|---|
| Claude Code | `~/.claude/projects/<cwd with non-alphanumerics as ->/<session>.jsonl` (+ `<session>/` subagents) |
| Codex | `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-*.jsonl`, matched on `cwd` |
| opencode | `opencode session list` / `opencode export` |

### Inside

[Inside](https://inside.speed-control.cz) stores bundles on tasks. With
`INSIDE_URL` and `INSIDE_MCP_TOKEN` set:

```sh
flightlog push abc.flightlog.zip --task PROJ-42
flightlog pull PROJ-42 && flightlog restore <file>
```

Any other tracker can accept bundles the same way: hand out a presigned upload
URL and use `flightlog push --url`.

## Teach your agent

flightlog ships two skills, `flightlog-export` and `flightlog-import`, that
tell an agent how to save the current session and how to pick one up again.

```sh
flightlog skills install            # every agent found on this machine
```

Or add this repository as a plugin marketplace:

| Agent | Command |
|---|---|
| Claude Code | `claude plugin marketplace add skrcka/flightlog` then `claude plugin install flightlog@flightlog` |
| Codex | `codex plugin marketplace add skrcka/flightlog` |
| Copilot CLI | `copilot plugin marketplace add skrcka/flightlog` |
| Gemini CLI | `gemini extensions install https://github.com/skrcka/flightlog` |
| Cursor | Settings → Plugins → add marketplace `https://github.com/skrcka/flightlog` |

opencode, Cursor and Gemini CLI also read `~/.agents/skills`, where
`flightlog skills install` always puts a copy.

## Install

- **Script**, x86-64 and arm64: `install.sh` for macOS and Linux, `install.ps1`
  for Windows (see the top). `FLIGHTLOG_VERSION` pins a version,
  `FLIGHTLOG_INSTALL_DIR` changes the target (`~/.local/bin`, on Windows
  `%LOCALAPPDATA%\flightlog\bin`, which the script adds to the user `PATH`).
- **Release archives:** [GitHub releases](https://github.com/skrcka/flightlog/releases),
  each with a `.sha256`.
- **Cargo:** `cargo install flightlog`
- **From source:** `cargo build --release`

## Status

Early. Converters track formats the tools change without notice; please open
an issue with the tool version when an export looks wrong. Planned: Cursor and
Gemini CLI, cross-tool resume, more redaction rules.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
