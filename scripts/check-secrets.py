#!/usr/bin/env python3
"""Run a pinned, checksum-verified Gitleaks binary in Linux CI."""
import hashlib, io, pathlib, subprocess, tarfile, tempfile, urllib.request, shutil, os
VERSION='8.30.1'
SHA256='551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb'
url=f'https://github.com/gitleaks/gitleaks/releases/download/v{VERSION}/gitleaks_{VERSION}_linux_x64.tar.gz'
with urllib.request.urlopen(url,timeout=30) as response: data=response.read(50*1024*1024)
if hashlib.sha256(data).hexdigest()!=SHA256:raise SystemExit('Gitleaks checksum mismatch')
with tempfile.TemporaryDirectory() as tmp:
    exe=pathlib.Path(tmp)/'gitleaks'
    with tarfile.open(fileobj=io.BytesIO(data),mode='r:gz') as archive:
        exe.write_bytes(archive.extractfile('gitleaks').read())
    exe.chmod(0o700)
    subprocess.run([str(exe),'git','.','--redact=100','--no-banner','--config','.gitleaks.toml'],check=True)
    source=pathlib.Path(tmp)/'source';source.mkdir()
    for raw in subprocess.check_output(['git','ls-files','-z','--cached','--others','--exclude-standard']).split(b'\0'):
        if raw:
            p=pathlib.Path(os.fsdecode(raw))
            if p.is_file():
                dest=source/p;dest.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(p,dest)
    subprocess.run([str(exe),'dir','.','--redact=100','--no-banner','--config','.gitleaks.toml'],cwd=source,check=True)
