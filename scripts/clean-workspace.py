#!/usr/bin/env python3
"""Remove disposable project outputs; preview unless --apply is supplied.

Does not touch installed applications, global Cargo caches, account data,
configuration, vendored sources or Git history. Do not run during a build.
"""
import argparse
import json
from pathlib import Path
import shutil
import subprocess


ROOT = Path(__file__).resolve().parents[1]


def git(*arguments, directory=ROOT):
    return subprocess.check_output(["git", "-C", str(directory), *arguments])


def clean(args):
    paths = [ROOT / "target", ROOT / ".prep"]
    for directory in ("scripts", "tests", "docs"):
        paths.extend((ROOT / directory).rglob("__pycache__"))
    if args.references:
        sources = json.loads((ROOT / "prep/upstreams.json").read_text())["sources"]
        for name in ("zapfast", "whatsapp-rust"):
            path = ROOT / ".references" / name
            if not path.exists():
                continue
            if path.resolve() != path:
                raise ValueError(f"Refusing reference symlink: {path}")
            if git("status", "--porcelain", "--untracked-files=all", directory=path):
                raise ValueError(f"Reference has local changes; preserving {path}")
            revision = git("rev-parse", "HEAD", directory=path).decode().strip()
            if revision != sources[name]["revision"]:
                raise ValueError(f"Reference is not at its recorded revision: {path}")
            paths.append(path)
    if args.desktop_reference:
        paths.append(ROOT / "whatsapp_desktop")

    # Validate the complete list before removing anything. Never follow an
    # output-directory symlink or remove tracked source through this helper.
    for path in paths:
        if path.resolve() != path:
            raise ValueError(f"Refusing output symlink: {path}")
        if path.exists() and not path.is_dir():
            raise ValueError(f"Expected a directory: {path}")
        if git("ls-files", "-z", "--", str(path.relative_to(ROOT))):
            raise ValueError(f"Directory contains tracked source: {path}")
    for path in paths:
        if path.exists():
            print(f"{'Removing' if args.apply else 'Would remove'} {path.relative_to(ROOT)}", flush=True)
            if args.apply:
                shutil.rmtree(path)
    references = ROOT / ".references"
    if args.apply and args.references and references.is_dir() and not any(references.iterdir()):
        references.rmdir()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--apply", action="store_true", help="Remove the listed directories")
    parser.add_argument("--references", action="store_true", help="Also remove clean pinned reference checkouts")
    parser.add_argument("--desktop-reference", action="store_true", help="Also remove the supplied Windows reference package")
    args = parser.parse_args()
    try:
        clean(args)
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        raise SystemExit(str(error)) from error


if __name__ == "__main__":
    main()
