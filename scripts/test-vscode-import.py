#!/usr/bin/env python3
"""Optional native VS Code chat import/export test; requires a desktop and `code`.

Uses an isolated profile and extension test host. No sign-in or model turn.
Second argument may name another installed VS Code executable.
"""
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile

binary = str(pathlib.Path(sys.argv[1]).resolve())
editor = shutil.which(sys.argv[2] if len(sys.argv) > 2 else "code")
if editor is None:
    sys.exit("Native VS Code test requires an installed editor executable (code, or pass its path).")

with tempfile.TemporaryDirectory(prefix="flightlog-vscode-") as tmp:
    root = pathlib.Path(tmp).resolve()
    env = dict(os.environ)
    env.pop("FLIGHTLOG_REDACT_FILE", None)
    (root / "source.json").write_text(json.dumps({"responderUsername": "Copilot", "requests": [
        {"message": "Remember cobalt migration", "response": [{"value": "Cobalt migration ready"}]}
    ]}))
    for args in (("export", "--tool", "copilot-vscode", "--input", "source.json", "--no-native", "-o", "test.zip"),
                 ("restore", "test.zip", "--to", "copilot-vscode")):
        subprocess.run([binary, *args], cwd=root, env=env, capture_output=True,
                       text=True, check=True, timeout=30)
    imported = next((root / ".flightlog/imports").glob("*.copilot-vscode.json"))
    exported = root / "editor-export.json"
    env["FLIGHTLOG_TEST_IMPORT"] = str(imported)
    env["FLIGHTLOG_TEST_EXPORT"] = str(exported)
    extension = root / "test-extension"
    extension.mkdir()
    (extension / "package.json").write_text(json.dumps({
        "name": "flightlog-compatibility-test", "version": "0.0.1", "publisher": "flightlog",
        "engines": {"vscode": "^1.85.0"}, "main": "./extension.js",
        "activationEvents": ["onStartupFinished"]
    }))
    (extension / "extension.js").write_text("exports.activate = () => {};\n")
    (extension / "test.js").write_text(r'''
const vscode = require('vscode');
const fs = require('node:fs');
const assert = require('node:assert/strict');
exports.run = async () => {
    const commands = await vscode.commands.getCommands(true);
    assert(commands.includes('workbench.action.chat.import'), 'Editor has no chat import command');
    await vscode.commands.executeCommand('workbench.action.chat.import', {
        inputPath: vscode.Uri.file(process.env.FLIGHTLOG_TEST_IMPORT), target: 'chatViewPane'
    });
    await vscode.commands.executeCommand('workbench.action.chat.export',
        vscode.Uri.file(process.env.FLIGHTLOG_TEST_EXPORT));
    const data = JSON.parse(fs.readFileSync(process.env.FLIGHTLOG_TEST_EXPORT, 'utf8'));
    assert.equal(data.requests.length, 2, 'Expected handoff and original request');
    const message = data.requests[1].message;
    assert((typeof message === 'string' ? message : message.text).startsWith('Remember cobalt migration'));
    assert(data.requests[1].response.some(part =>
        typeof part.value === 'string' && part.value.startsWith('Cobalt migration ready')));
};
''')
    profile = root / "profile"
    (profile / "User").mkdir(parents=True)
    (profile / "User/settings.json").write_text(json.dumps({
        "telemetry.telemetryLevel": "off", "extensions.autoUpdate": False,
        "extensions.autoCheckUpdates": False, "update.mode": "none", "workbench.startupEditor": "none"
    }))
    result = subprocess.run([editor, str(root), "--new-window", "--skip-welcome", "--skip-release-notes",
                             "--disable-workspace-trust", f"--user-data-dir={profile}",
                             f"--extensions-dir={root / 'extensions'}",
                             f"--extensionDevelopmentPath={extension}",
                             f"--extensionTestsPath={extension / 'test.js'}"],
                            cwd=root, env=env, text=True, capture_output=True, timeout=90)
    assert result.returncode == 0, (result.stdout + result.stderr)[-8000:]
    assert exported.is_file(), "Editor exited without executing the import/export test"
    print("VS Code imported and re-exported converted history with roles and order preserved; no model turn submitted.")
