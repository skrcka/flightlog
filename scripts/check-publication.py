#!/usr/bin/env python3
"""Scan publication inputs without printing private terms or matched contents.
Private inventory lives in PRIVATE_REFERENCE_TERMS or --terms-file, never source.
"""
import argparse, io, os, pathlib, re, subprocess, sys, tarfile, zipfile

def archive_files(path):
    if zipfile.is_zipfile(path):
        with zipfile.ZipFile(path) as archive:
            for item in archive.infolist():
                if not item.is_dir(): yield item.filename, archive.read(item)
    else:
        with tarfile.open(path, 'r:*') as archive:
            for item in archive:
                if item.isfile(): yield item.name, archive.extractfile(item).read()

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--terms-file',type=pathlib.Path)
    parser.add_argument('--require-policy',action='store_true')
    parser.add_argument('--archive',type=pathlib.Path,action='append',default=[])
    parser.add_argument('--history',action='store_true')
    args=parser.parse_args()
    policy=args.terms_file.read_text() if args.terms_file else os.environ.get('PRIVATE_REFERENCE_TERMS','')
    terms=[s.strip().lower() for s in policy.splitlines() if s.strip() and not s.lstrip().startswith('#')]
    if args.require_policy and not terms:
        print('Publication blocked: configure the private-reference inventory.',file=sys.stderr);return 1
    if any(len(t)<3 for t in terms):
        print('Policy terms must have at least three characters.',file=sys.stderr);return 1
    needles=[b for t in terms for b in (t.encode(),t.encode('utf-16le'))]
    failed=False;count=0
    def check(name,data):
        nonlocal failed,count
        count+=1
        if any(term in data.lower() or term in name.lower().encode() for term in needles):
            # File names can themselves be sensitive: report only their ordinal.
            print(f'Private reference found in publication item {count}.',file=sys.stderr);failed=True
    if args.archive:
        for p in args.archive:
            for name,data in archive_files(p): check(name,data)
    elif args.history:
        objects=subprocess.check_output(['git','rev-list','--objects','--all']).splitlines()
        for line in objects:
            sha=line.split(b' ',1)[0].decode()
            kind=subprocess.check_output(['git','cat-file','-t',sha]).strip()
            if kind in (b'blob', b'commit', b'tag'):
                check(line.decode(errors='replace'),subprocess.check_output(['git','cat-file',kind.decode(),sha]))
        check('refs', subprocess.check_output(['git','for-each-ref','--format=%(refname)']))
    else:
        paths=subprocess.check_output(['git','ls-files','-z','--cached','--others','--exclude-standard']).split(b'\0')
        for raw in paths:
            if raw:
                p=pathlib.Path(os.fsdecode(raw))
                if p.is_file():check(str(p),p.read_bytes())
    print(f'Scanned {count} publication items; '+('FAILED' if failed else 'passed'))
    return int(failed)
if __name__=='__main__':sys.exit(main())
