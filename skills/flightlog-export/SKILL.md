---
name: flightlog-export
description: Save the current coding-agent session (Claude Code, Codex, opencode, Gemini CLI or Cursor) as a portable, redacted flightlog bundle, and optionally upload it to a task tracker or presigned URL. Use when the user asks to save, export, hand off, share or attach this session or its context (e.g. "export this session", "attach this session to PROJ-42").
---

# Export this session with flightlog

`flightlog` turns the session of the current directory into a `.flightlog.zip`
bundle: the conversation as an ATIF trajectory, a summary, a redaction report,
and the tool's own session files so it can be resumed later.

If `flightlog --version` fails, install it (a single binary):

```bash
curl -fsSL https://flightlog.sh/install.sh | sh     # macOS, Linux
```

```powershell
irm https://flightlog.sh/install.ps1 | iex          # Windows
```

## Steps

1. **Write the summary** into `summary.json` from what you know of this
   session. It is the first thing the next person or agent reads; be concrete
   and never include secrets.
   ```json
   {
     "goal": "what the session set out to do",
     "state": "where it stands now: done, blocked, what is left",
     "decisions": ["decisions made and why"],
     "open_questions": ["what is still undecided"],
     "next_steps": ["concrete next actions"],
     "files_touched": ["paths changed"],
     "language": "en"
   }
   ```
   `goal` and `state` are required.
2. **Export**:
   ```bash
   flightlog export --summary summary.json -o session.flightlog.zip
   ```
   It picks the newest session for this directory. `flightlog list` shows
   the others; pick one with `--tool claude|codex|opencode|gemini|cursor --session <id>`.
   `--no-native` leaves out the tool's own files (the bundle can then be read
   but not resumed).
3. **Show the user the redaction report** it printed (placeholders like
   `[REDACTED:password:1]`, counts and files). Ask whether anything else
   should not be shared before going further.
4. **Share**, only once the user agrees:
   - Attach to an [Inside](https://inside.speed-control.cz) task (needs
     `INSIDE_URL` and `INSIDE_MCP_TOKEN`):
     `flightlog push session.flightlog.zip --task PROJ-42`
   - Upload to a presigned URL: `flightlog push session.flightlog.zip --url '<url>'`
   - Or just tell the user where the file is.
5. Delete `summary.json` (and the zip, once uploaded) when done.

`flightlog inspect session.flightlog.zip` shows what a bundle contains;
`flightlog validate` checks it against the format.

**The format.** A bundle is a ZIP with `manifest.json`, `trajectory.json`
([ATIF v1.8](https://github.com/harbor-framework/harbor/blob/main/rfcs/0001-trajectory-format.md))
and the tool's own session files under `native/`. Full spec:
https://github.com/skrcka/flightlog/blob/main/SPEC.md
