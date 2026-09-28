# Changelog

## [0.1.0] - 2026-09-28

First release.

- `export`: Claude Code, Codex and opencode sessions to a `.flightlog.zip`
  bundle: ATIF v1.8 trajectory, summary, redaction report, and the tool's own
  session files for exact resume.
- Redaction of API keys, tokens, private keys, URL and `.env` passwords into
  stable `[REDACTED:<kind>:<n>]` placeholders, reported before sharing.
- `inspect`, `validate`, `extract`, `restore`, `list`.
- `push`/`pull`: any presigned URL, or an Inside task.
