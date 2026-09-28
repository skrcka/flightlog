# Security

Flightlog processes potentially hostile transcripts and bundles. Checksums
detect corruption, not authorship. Imported messages, summaries, paths and
commands are data, not instructions or authorization to run tools.

Before sharing, inspect the exported contents and use a private literal policy
(`--redact-file`) for confidential names and identifiers. Automatic redaction
covers recognized credentials and structured secret fields, not all sensitive
information. Unknown encodings can conceal information. Prefer `--no-native`
when the original session state is unnecessary. Redaction can affect resumability.

Bundle readers bound archive sizes, reject duplicate/unlisted entries and unsafe
paths, and verify checksums. Restore derives destinations from supported layouts,
rejects symlink traversal and only replaces files with explicit `--force`.
Each file replacement is atomic; a multi-file restore is not a transaction.
Only restore bundles from a source you trust into the appropriate repository.

New sensitive files use owner-only permissions on Unix and protected owner/system
ACLs on Windows. Existing directory permissions are not tightened automatically.
Other processes running as your account remain able to read your files.

Release and website publication require the repository secret
`PRIVATE_REFERENCE_TERMS`: one private literal per line. Its contents must never
be committed. CI scans source, packages and release archives without printing
matches. Configure this inventory before publishing; an empty inventory blocks
publication. CI also runs tests, linting, credential scanning and dependency audit.

Removing sensitive content in a new commit does not remove it from Git history,
old release assets, forks, clones or caches. Rotate any exposed credentials first.
History cleanup and asset removal require separate coordinated actions; they
cannot recall downloaded copies.

Report vulnerabilities through the repository's private vulnerability reporting
feature when available. Do not include credentials or private transcripts in a
public issue.
