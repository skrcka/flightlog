#!/usr/bin/env python3
"""Optional POSIX Claude Code UI smoke check in a disposable store; no model turn."""
import json
import os
import pathlib
import pty
import re
import select
import subprocess
import sys
import tempfile
import time

binary = str(pathlib.Path(sys.argv[1]).resolve())
with tempfile.TemporaryDirectory(prefix="flightlog-claude-") as tmp:
    root = pathlib.Path(tmp).resolve()
    config = root / "claude"
    config.mkdir()
    env = dict(os.environ, CLAUDE_CONFIG_DIR=str(config),
               ANTHROPIC_API_KEY="flightlog-test-placeholder",
               CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC="1", DISABLE_AUTOUPDATER="1",
               TERM="xterm-256color")
    for key in ("FLIGHTLOG_REDACT_FILE", "CLAUDE_CODE_OAUTH_TOKEN", "ANTHROPIC_AUTH_TOKEN",
                "ANTHROPIC_BASE_URL", "CLAUDE_CODE_USE_BEDROCK", "CLAUDE_CODE_USE_VERTEX",
                "CLAUDE_CODE_USE_FOUNDRY"):
        env.pop(key, None)
    (root / "chat.json").write_text(json.dumps({"responderUsername": "Copilot", "requests": [
        {"message": "Remember cobalt migration", "response": [{"value": "Cobalt migration ready"}]}
    ]}))
    for args in (("export", "--tool", "copilot-vscode", "--input", "chat.json", "--no-native", "-o", "test.zip"),
                 ("restore", "test.zip", "--to", "claude")):
        result = subprocess.run([binary, *args], cwd=root, env=env, text=True,
                                capture_output=True, check=True, timeout=30)
    session_id = re.search(r"claude --resume ([a-f0-9-]+)", result.stdout)[1]
    (config / ".claude.json").write_text(json.dumps({
        "hasCompletedOnboarding": True, "theme": "dark", "hasAcknowledgedCostThreshold": True,
        "customApiKeyResponses": {"approved": [env["ANTHROPIC_API_KEY"][-20:]], "rejected": []},
        "projects": {str(root): {"hasTrustDialogAccepted": True, "hasCompletedProjectOnboarding": True}}
    }))
    master, slave = pty.openpty()
    process = subprocess.Popen(["claude", "--bare", "--resume", session_id,
                                "--ax-screen-reader", "--setting-sources", ""],
                               cwd=root, env=env, stdin=slave, stdout=slave,
                               stderr=slave, start_new_session=True)
    os.close(slave)
    output = b""
    deadline = time.monotonic() + 20
    passed = False
    try:
        while time.monotonic() < deadline:
            if select.select([master], [], [], 0.2)[0]:
                try:
                    output += os.read(master, 65536)
                except OSError:
                    break
            text = re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", output.decode(errors="replace"))
            compact = re.sub(r"\s+", "", text)
            if "Remembercobaltmigration" in compact and "Cobaltmigrationready" in compact:
                passed = True
                break
            if process.poll() is not None:
                break
    finally:
        process.terminate()
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        os.close(master)
    assert passed, "Claude did not render both imported messages:\n" + text[-5000:]
    print("Claude Code resumed and rendered imported user/assistant history; no model turn submitted.")
