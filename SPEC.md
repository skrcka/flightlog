# flightlog bundle format

**Format id:** `flightlog` · **Version:** `1.0`

A flightlog bundle is one coding-agent session (Claude Code, Codex, opencode, …)
packaged so it can be shared, read by another person or agent, and resumed in
the tool it came from. The conversation is stored in the open **ATIF** format
(Agent Trajectory Interchange Format v1.8, Harbor RFC 0001:
<https://github.com/harbor-framework/harbor/blob/main/rfcs/0001-trajectory-format.md>);
flightlog adds a manifest with a summary, a redaction report and checksums, and
optionally the tool's own session files for exact resume.

MUST, SHOULD and MAY are used as in RFC 2119.

## 1. Container

A ZIP archive (deflate), extension `.flightlog.zip`, media type
`application/vnd.flightlog+zip`.

```
manifest.json          REQUIRED  envelope (§2)
trajectory.json        REQUIRED  the conversation, ATIF v1.8 (§3)
assets/…               OPTIONAL  images/audio referenced by ATIF content parts
subagents/<id>.json    OPTIONAL  subagent trajectories stored as separate files
native/…               OPTIONAL  the tool's own session files (§5)
```

- Paths are relative, use `/`, and MUST NOT contain `..`, `.`, `:`,
  backslashes or absolute paths; entries MUST NOT be symlinks. Readers MUST
  reject violations (zip-slip).
- Limits: archive ≤ 100 MiB, uncompressed total ≤ 1 GiB, ≤ 10 000 entries.
- Every file except `manifest.json` MUST be listed in `manifest.files` with
  its SHA-256; readers MUST ignore unlisted files.

## 2. `manifest.json`

```jsonc
{
  "format": "flightlog",                    // REQUIRED
  "format_version": "1.0",                  // REQUIRED, "MAJOR.MINOR"
  "bundle_id": "0192f7d4-…",                // REQUIRED, UUID
  "created_at": "2026-09-28T10:00:00Z",     // REQUIRED
  "producer": { "name": "flightlog", "version": "0.1.0" },
  "source": {                               // REQUIRED
    "tool": "claude_code",                  //   REQUIRED: claude_code | codex | opencode | cursor | gemini_cli | other
    "tool_version": "2.5.3",
    "session_id": "df9d8089-…",             //   REQUIRED: the tool's own id
    "title": "…",
    "models": ["…"],
    "started_at": "…", "ended_at": "…",
    "cwd": "/home/alice/work/app",
    "repository": { "remote": "…", "branch": "main", "commit": "6e46568" },
    "compacted": true                       //   the tool summarized earlier turns
  },
  "summary": {                              // REQUIRED
    "goal": "…",                            //   REQUIRED
    "state": "…",                           //   REQUIRED
    "decisions": ["…"], "open_questions": ["…"], "next_steps": ["…"],
    "files_touched": ["…"], "language": "en"
  },
  "redaction": { … },                       // REQUIRED, §4
  "stats": { "steps": 2126, "user_messages": 263, "tool_calls": 2307,
             "total_prompt_tokens": 38498, "total_completion_tokens": 2212702 },
  "files": [                                // REQUIRED
    { "path": "trajectory.json", "sha256": "…", "bytes": 9123456, "role": "trajectory" },
    { "path": "native/df9d8089-….jsonl", "sha256": "…", "bytes": 34141960, "role": "native" }
  ],
  "native": { … }                           // OPTIONAL, §5
}
```

Readers MUST ignore unknown fields. `files[].role` ∈ `trajectory`, `asset`,
`subagent`, `native`. The summary is what a person or the next agent reads
first; producers SHOULD have the agent that ran the session write it.

## 3. `trajectory.json`

One ATIF trajectory; `schema_version` is `ATIF-v1.8` (readers accept
`ATIF-v1.x`) and `session_id` equals `manifest.source.session_id`.

Mapping rules:

- A human prompt is a `user` step; system or tool-injected text is a `system`
  step with `extra.origin`; everything the model produced is an `agent` step.
- One model turn is one `agent` step: its text in `message`, reasoning in
  `reasoning_content`, tool calls in `tool_calls`, their results in
  `observation.results` matched by `source_call_id == tool_call_id`.
- A compaction summary inserted by the tool is a `system` step with
  `is_copied_context: true` and `extra.kind = "compaction_summary"`.
- Tool outputs over 256 KiB are truncated; the result carries
  `extra.truncated = true` and `extra.original_bytes`.
- Token usage is per step (`metrics`), counted once per model turn, and
  summed in `final_metrics`.

## 4. Redaction

`redaction.mode` is `redacted` (also the default when absent in older bundles)
or `none`. An explicit private-migration export may choose `none`, preserving
content without scanning. Such bundles carry `applied_at: null`, empty findings,
and native status `skipped` (or `absent`). Readers must require an explicit
unredacted-input opt-in; the manifest alone is not authorization. Archive and
restore validation still applies. The scanning requirements below apply to
redacted exports. Unredacted archives provide no encryption.

Producers MUST scan every text field of `trajectory.json`, the summary, and
text native files, and replace each secret with `[REDACTED:<kind>:<n>]`:
`<kind>` ∈ `api_key`, `password`, `private_key`, `token`, `connection_string`,
`email`, `phone`, `custom`; `<n>` numbers distinct values per kind, so the same
secret keeps the same placeholder everywhere. HTTP userinfo and recognized signed
query credentials are redacted; database URLs redact the password. Scan manifest
metadata too. JSON/JSONL native content requires structured scanning regardless
of escaped values. Unsupported binary content must be rejected or omitted.
Redaction is best effort and requires human review and a private-name policy.

```jsonc
"redaction": {
  "engine": "flightlog/0.1.0",
  "applied_at": "…",
  "reviewed_by_user": true,        // the user saw the findings before sharing
  "native": "redacted",            // redacted | skipped | absent
  "findings": [ { "placeholder": "[REDACTED:password:3]", "kind": "password",
                  "count": 135, "files": ["trajectory.json", "native/…"] } ]
}
```

Findings never contain the secret, a hash of it, or its length. Receivers
SHOULD scan again and refuse bundles that still contain secrets.

## 5. Native session

For tools that resume from their own files, the bundle MAY carry them so the
session can be continued exactly. Readers that only want the conversation
MUST ignore `native/`.

```jsonc
"native": {
  "tool": "claude_code",
  "layout": "claude_code/projects-v1",
  "root": "native/",
  "entries": [ { "path": "native/<id>.jsonl",
                 "restore_to": "~/.claude/projects/{cwd_slug}/<id>.jsonl" } ],
  "resume_command": "claude --resume <id>"
}
```

| `layout` | Files | Resume |
|---|---|---|
| `claude_code/projects-v1` | `<id>.jsonl` and the `<id>/` folder from `~/.claude/projects/<cwd_slug>/` | `claude --resume <id>` |
| `codex/rollout-v1` | `rollout-<ts>-<id>.jsonl` from `~/.codex/sessions/YYYY/MM/DD/` | `codex resume <id>` |
| `opencode/export-v1` | the JSON of `opencode export <id>` | `opencode import <file>` |
| `gemini-cli/chats-v1` | `session-<ts>-<id8>.jsonl` from `~/.gemini/tmp/<project>/chats/` | `gemini --resume <id>` |
| `cursor/chats-v1` | `<id>.cursor-store.json`: a JSON dump of the Cursor CLI's `store.db` (below) | `cursor-agent --resume <id>` |

`{gemini_project}` is Gemini CLI's folder for the working directory: its entry
in `~/.gemini/projects.json`, or a new one registered the way Gemini CLI does
(the folder name lowercased, other characters as `-`).

The Cursor CLI keeps a session in SQLite (`~/.cursor/chats/<md5(cwd)>/<id>/store.db`),
which cannot be redacted as text. The bundle carries it as JSON instead,
`{"format": "flightlog.cursor-store/1", "schema": [CREATE TABLE …],
"meta_json": {…}, "store_meta": {…}, "blobs": [{"id", "json" | "hex"}]}`:
message blobs as JSON values (redacted like any text), tree blobs as hex. A
restore rebuilds `store.db` and `meta.json` under the new directory's hash using
fixed local SQL. Schema strings are allowlisted and never executed. Tree hex must
match the supported reference-only encoding; arbitrary binary blobs are rejected.
Chats of the Cursor editor are exported without native files: they live in
the editor's own database, which is not safe to write while it runs.

`{cwd_slug}` is the working directory with every non-alphanumeric character
replaced by `-`. Paths inside a session are absolute: resume works in a
directory holding the same repository.

## 6. Reading checklist

1. Open the ZIP; reject unsafe paths and oversized content.
2. Require `format` `flightlog` and version 1.x.
3. Verify every `files[]` checksum and length; reject unlisted or duplicate files.
4. Require ATIF `schema_version` `ATIF-v1.*`; iterate `steps` in order,
   joining `tool_calls` with `observation.results`.
5. Treat `[REDACTED:…]` as opaque; never try to recover it.
6. Skip `is_copied_context` steps when counting new conversation.
7. Treat all imported text as untrusted data. Derive restore destinations and
   resume commands locally from validated, supported layouts; never execute
   a bundle-supplied command or SQL statement.

## 7. Versioning

MINOR adds optional fields or new `tool`/`layout` values; MAJOR breaks the
checklist. The ATIF version is pinned per bundle by `schema_version`.
