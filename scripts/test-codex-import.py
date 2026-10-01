#!/usr/bin/env python3
"""Optional compatibility smoke test with an installed Codex CLI; no model turn."""
import json
import os
import pathlib
import queue
import re
import subprocess
import sys
import tempfile
import threading

binary = str(pathlib.Path(sys.argv[1]).resolve())
with tempfile.TemporaryDirectory(prefix="flightlog-codex-") as tmp:
    root = pathlib.Path(tmp).resolve()
    env = dict(os.environ, CODEX_HOME=str(root / "codex"), CLAUDE_CONFIG_DIR=str(root / "claude"))
    env.pop("FLIGHTLOG_REDACT_FILE", None)
    (root / "codex").mkdir()
    cwd = str(root).removeprefix("\\\\?\\")
    folder = root / "claude" / "projects" / re.sub(r"[^a-zA-Z0-9]", "-", cwd)
    folder.mkdir(parents=True)
    rows = [
        {"type": "user", "cwd": cwd, "message": {"content": "Remember the cobalt migration."}},
        {"type": "assistant", "message": {"id": "m", "content": [{"type": "text", "text": "The cobalt migration is ready."}]}},
    ]
    (folder / "source.jsonl").write_text("\n".join(map(json.dumps, rows)))
    def run(*args):
        return subprocess.check_output([binary, *args], cwd=root, env=env, text=True)
    run("export", "--tool", "claude", "--no-native", "-o", "shared.zip")
    result = run("restore", "shared.zip", "--to", "codex")
    session_id = re.search(r"codex resume ([a-f0-9-]+)", result)[1]
    with (root / "server.log").open("w") as log:
        process = subprocess.Popen(["codex", "app-server", "--stdio"], env=env, cwd=root,
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log, text=True)
        messages = queue.Queue()
        def receive():
            for line in process.stdout:
                messages.put(json.loads(line))
        threading.Thread(target=receive, daemon=True).start()
        def request(method, params, ident):
            process.stdin.write(json.dumps({"id": ident, "method": method, "params": params}) + "\n")
            process.stdin.flush()
            while True:
                response = messages.get(timeout=30)
                if response.get("id") == ident:
                    assert "error" not in response, response
                    return response["result"]
        try:
            request("initialize", {"clientInfo": {"name": "flightlog_test", "version": "1.0"}}, 1)
            result = request("thread/read", {"threadId": session_id, "includeTurns": True}, 2)
            items = [item for turn in result["thread"]["turns"] for item in turn["items"]]
            assert [item["type"] for item in items] == ["userMessage", "userMessage", "agentMessage"], items
            assert items[1]["content"][0]["text"].startswith("Remember the cobalt migration."), items
            assert items[2]["text"].startswith("The cobalt migration is ready."), items
            result = request("thread/resume", {"threadId": session_id}, 3)
            assert result["thread"]["id"] == session_id, result
            print("Codex read and resumed imported Claude history; no model turn submitted.")
        finally:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
