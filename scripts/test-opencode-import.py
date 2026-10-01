#!/usr/bin/env python3
"""Optional native opencode import/export smoke test in isolated stores; no model turn."""
import json
import os
import pathlib
import subprocess
import sys
import tempfile

binary = str(pathlib.Path(sys.argv[1]).resolve())
with tempfile.TemporaryDirectory(prefix="flightlog-opencode-") as tmp:
    root = pathlib.Path(tmp).resolve()
    env = dict(os.environ, XDG_DATA_HOME=str(root / "data"),
               XDG_CONFIG_HOME=str(root / "config"), XDG_CACHE_HOME=str(root / "cache"),
               XDG_STATE_HOME=str(root / "state"), OPENCODE_DISABLE_MODELS_FETCH="true",
               OPENCODE_DISABLE_AUTOUPDATE="true")
    env.pop("FLIGHTLOG_REDACT_FILE", None)

    def run(*args):
        result = subprocess.run(args, cwd=root, env=env, text=True,
                                capture_output=True, timeout=45)
        assert result.returncode == 0, result.stderr
        return result.stdout

    (root / "chat.json").write_text(json.dumps({"responderUsername": "Copilot", "requests": [
        {"message": "Remember cobalt migration", "response": [{"value": "Cobalt migration ready"}]}
    ]}))
    run(binary, "export", "--tool", "copilot-vscode", "--input", "chat.json", "--no-native", "-o", "test.zip")
    run(binary, "restore", "test.zip", "--to", "opencode")
    path = next((root / ".flightlog/imports").glob("*.opencode.json"))
    source = json.loads(path.read_text())
    run("opencode", "import", str(path))
    # opencode's listing spans directories; Flightlog must filter automatic picks.
    assert source["info"]["id"] in run(binary, "list", "--tool", "opencode")
    elsewhere = root / "scratchpad"
    elsewhere.mkdir()
    assert source["info"]["id"] not in run(binary, "list", "--tool", "opencode", "--cwd", str(elsewhere))
    run(binary, "export", "--tool", "opencode", "--session", source["info"]["id"],
        "--cwd", str(elsewhere), "--no-native", "-o", "explicit.zip")
    restored = json.loads(run("opencode", "export", source["info"]["id"]))
    texts = [part["text"] for message in restored["messages"] for part in message["parts"] if part["type"] == "text"]
    assert len(texts) == 3, texts
    assert "Remember cobalt migration" in texts[1], texts
    assert "Cobalt migration ready" in texts[2], texts
    print("Installed opencode imported and exported converted history in order; no model turn submitted.")
