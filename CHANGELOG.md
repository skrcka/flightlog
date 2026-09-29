# Changelog

## [0.2.2] - 2026-09-29

- Add `export --no-redact` for private migration and `--allow-unredacted` on input.
  Explicitly mark these bundles and preserve native bytes without redaction.
- Add independent `--skip-path-checks`, `--skip-size-checks`, `--skip-checksums`,
  `--skip-format-checks`, `--skip-content-checks`, `--overwrite` and `--allow-http`
  overrides. `--yolo` combines all overrides and disables export redaction.
  Normal commands retain the security defaults.

- Confine restores to known agent layouts; derive resume commands locally and
  reject unsafe paths, aliases, undeclared files and incorrect ZIP sizes.
- Write sensitive output privately and atomically. Extraction requires a new
  directory; export and download refuse to overwrite existing files.
- Redact metadata, structured credentials and all supported native text.
  Omit repository/cwd metadata by default; add `--include-metadata` to include it.
  Reject unsupported binary/encoded native content; `--no-native` exports only
  the readable conversation. Redaction remains best effort; review before sharing.
- Add `--redact-file` / `FLIGHTLOG_REDACT_FILE` for private names and domains.
- Require HTTPS, disable redirects, add transfer deadlines and `--url-file`.
- Use a fixed Cursor database schema and SQLite 3.53.2; exclude unreachable blobs.
- Gate releases on tests, dependency/secret scans and a private-reference policy;
  inspect archives before upload and publish build attestations.
- Fix private Windows file and directory permissions and Windows session paths.
- Rust 1.88 or newer is required.

## [0.2.1] - 2026-09-28

- Clearer `--help`: a one-line summary per command, longer descriptions and
  examples with `--help`, commands in workflow order, the agent skills
  mentioned up front.
- Man pages (`man flightlog`, `man flightlog-export`, …) in the release
  archives, installed by `install.sh` to `~/.local/share/man`.

## [0.2.0] - 2026-09-28

- `push` takes only `--url`; `pull <url>` downloads and validates a bundle.
  Tracker-specific uploads are left to the agent and the tracker's own tools.
  `export` prints the archive's size and SHA-256.
- Only the `flightlog` format id is accepted.
- Windows: release builds for x86-64 and arm64, `install.ps1`
  (`irm https://flightlog.sh/install.ps1 | iex`), path handling for
  `\\?\` prefixes, drive-letter case and opencode's npm shim.
- Gemini CLI: export and restore (`gemini --resume <id>`), JSONL and legacy
  JSON sessions.
- Cursor: export and restore of Cursor CLI sessions (`cursor-agent --resume
  <id>`), carried as a redactable JSON dump of `store.db`; editor chats are
  exported read-only.
- `skills install|show`: the `flightlog-export` and `flightlog-import` agent
  skills for Claude Code, Codex, opencode, Copilot, Cursor and Gemini CLI.
  The repository is also a plugin marketplace (Claude Code, Codex, Copilot,
  Cursor) and a Gemini CLI extension.

## [0.1.0] - 2026-09-28

First release.

- `export`: Claude Code, Codex and opencode sessions to a `.flightlog.zip`
  bundle: ATIF v1.8 trajectory, summary, redaction report, and the tool's own
  session files for exact resume.
- Redaction of API keys, tokens, private keys, URL and `.env` passwords into
  stable `[REDACTED:<kind>:<n>]` placeholders, reported before sharing.
- `inspect`, `validate`, `extract`, `restore`, `list`.
- `push`/`pull`: presigned upload and download URLs.
