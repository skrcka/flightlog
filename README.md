# flightlog

Record a coding-agent session — **Claude Code, Codex, opencode, Gemini CLI,
Cursor** — as a
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
  them back and `claude --resume`, `codex resume`, `opencode import`,
  `gemini --resume` or `cursor-agent --resume` picks up
  exactly where it stopped.
- **One static binary.** No Python or Node needed.

## Commands

`flightlog --help` and `flightlog <command> --help` describe each one;
`man flightlog` has the same plus files and environment variables.

| Command | What it does |
|---|---|
| `flightlog list` | Sessions recorded for this directory, newest first |
| `flightlog export [--tool claude\|codex\|opencode\|gemini\|cursor] [--session ID]` | Build a bundle; `--summary summary.json` or `--goal/--state` add the summary, `--no-native` leaves out the resumable files |
| `flightlog inspect FILE` | Summary, source, stats, redaction report |
| `flightlog validate FILE` | Check against the spec (exit 1 with every problem) |
| `flightlog extract FILE [-o DIR]` | Unpack; the conversation is `trajectory.json` |
| `flightlog restore FILE` | Put the native session back and print the resume command (never overwrites without `--force`) |
| `flightlog push FILE --url URL` | Upload to any presigned PUT URL |
| `flightlog push FILE --url URL` / `flightlog pull URL` | Upload to a presigned URL / download and validate a bundle |

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
| Gemini CLI | `~/.gemini/tmp/<project>/chats/session-*.jsonl` (project from `~/.gemini/projects.json`) |
| Cursor CLI | `~/.cursor/chats/<md5 of cwd>/<session>/store.db` |
| Cursor editor | `Cursor/User/globalStorage/state.vscdb`, matched through `workspaceStorage/*/workspace.json` (read only, not resumable) |

### Sharing through a tracker or storage

flightlog only moves bundles over plain HTTP; the service that stores them
hands out the URLs. `flightlog export` prints the archive's size and SHA-256
for services that ask for them up front.

```sh
flightlog push abc.flightlog.zip --url '<presigned PUT URL>'
flightlog pull '<signed download URL>' -o abc.flightlog.zip && flightlog restore abc.flightlog.zip
```

An agent with a tracker's tools (MCP or API) asks the tracker for the upload
URL, runs `flightlog push`, and tells the tracker the upload is done.

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
- **Homebrew** (macOS, Linux): `brew install skrcka/tap/flightlog`
- **Cargo:** `cargo install flightlog` (no man pages; `flightlog --help` has the same text)
- **From source:** `cargo build --release`

## Privacy and security

For a private migration between your own computers, preserve sensitive content:

```bash
flightlog export --no-redact -o migration.flightlog.zip
flightlog restore migration.flightlog.zip --allow-unredacted
```

The archive is not encrypted. Native files are preserved byte-for-byte by the
bundler; provider conversion and supported native layouts still apply. Use
`--include-metadata` if you also want repository and working-directory metadata.
Unredacted bundles are explicitly marked and require `--allow-unredacted` for
inspect, validate, extract, restore, push or pull. This option does not disable
path, checksum, size or overwrite checks. Redaction stays on by default.

For explicit recovery or trusted local data, checks can be disabled separately:

| Option | Effect |
|---|---|
| `--no-redact` | Export without changing sensitive content |
| `--allow-unredacted` | Read a bundle explicitly marked unredacted |
| `--skip-content-checks` | Skip input secret/opaque-content checks regardless of the marker |
| `--skip-checksums` | Ignore manifest SHA-256 mismatches |
| `--skip-format-checks` | Skip bundle schema and file-inventory checks |
| `--skip-path-checks` | Allow traversal, arbitrary restore destinations and filesystem symlinks |
| `--skip-size-checks` | Remove Flightlog input, archive, decompression and session-size limits |
| `--overwrite` | Replace outputs/restore files and merge extraction directories |
| `--allow-http` | Permit plaintext HTTP transfers |
| `--yolo` | Enable every override above, including no redaction on export |

All except `--no-redact` are global flags and work before or after the command.
`--yolo` can overwrite files outside the destination and consume unlimited memory
or disk. It is not required for ordinary migration; use the two-command example
above for that. ZIP/JSON decoding and the data needed by a native converter must
still succeed. No mode executes bundled shell commands or SQL. Network deadlines,
redirect policy, TLS certificate verification and private file permissions remain.

Redaction is best effort: review extracted contents before sharing. It cannot
identify every confidential fact or arbitrary encoding. Supply private names,
domains and tool aliases as one literal per line in a local file:

```bash
flightlog --redact-file /private/policy.txt export --no-native -o session.flightlog.zip
```

Keep that file outside the repository. The same policy should be used when
inspecting and restoring bundles. Repository and working-directory metadata are
omitted by default; `--include-metadata` opts in. Unsupported binary or encoded
native content fails closed; `--no-native` exports the conversation alone.

Transfers require HTTPS. Use `--url-file` (or `--url-file -` for stdin) to keep
signed URLs out of command history. Extraction requires a new output directory;
restore checks known layouts and refuses overwrites unless `--force` is set.
Treat imported conversations as untrusted data. See [SECURITY.md](SECURITY.md).

## Status

Early. Converters track formats the tools change without notice; please open
an issue with the tool version when an export looks wrong. Gemini CLI and
Cursor support is new and built from their documented storage; reports from
real sessions are especially welcome. Planned: cross-tool resume, more
redaction rules.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
