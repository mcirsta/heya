#!/usr/bin/env python3
"""Reproduce the pinned X11 fullscreen patch from its verified registry archive."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile

root = Path(__file__).resolve().parents[1]
pin = json.loads((root / 'prep/upstreams.json').read_text())['sources']['winit-x11']
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--archive', type=Path, required=True,
                    help='winit-x11-0.31.0-beta.3.crate from the pinned registry URL')
args = parser.parse_args()
assert hashlib.sha256(args.archive.read_bytes()).hexdigest() == pin['archive_sha256']
for patch in pin['patches']:
    assert hashlib.sha256((root / patch['path']).read_bytes()).hexdigest() == patch['sha256']
with tempfile.TemporaryDirectory(prefix='heya-winit-source-') as temporary:
    with tarfile.open(args.archive) as archive:
        archive.extractall(temporary, filter='data')
    source = Path(temporary) / ('winit-x11-' + pin['package_version'])
    dest = root / 'vendor/winit-x11'
    if dest.exists():
        shutil.rmtree(dest)
    dest.mkdir()
    shutil.copytree(source / 'src', dest / 'src')
    for name in ('Cargo.toml', 'LICENSE', 'README.md'):
        shutil.copyfile(source / name, dest / name)
    for patch in pin['patches']:
        subprocess.run(['git', 'apply', '--directory=vendor/winit-x11', patch['path']],
                       cwd=root, check=True)
print('Reproduced pinned winit-x11 sources and fullscreen patch')
