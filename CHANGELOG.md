# Changelog

## [0.2.0] - 2026-09-28

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
- `push`/`pull`: any presigned URL, or an Inside task.
