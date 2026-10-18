#!/usr/bin/env python3
"""Prepare/check macOS iconset PNGs derived directly from Heya's canonical SVG.

Generation needs rsvg-convert; ordinary macOS builds only need Python and Apple's
iconutil. Checked-in PNGs avoid adding an SVG renderer to the Mac build/runtime.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import struct
import subprocess

SLOTS = {f"icon_{points}x{points}{suffix}.png": points * scale
         for points in (16, 32, 128, 256, 512)
         for suffix, scale in (("", 1), ("@2x", 2))}


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def check(root):
    directory = root / "packaging/macos/Heya.iconset"
    manifest = json.loads((directory.parent / "icons.json").read_text())
    assert manifest["source_sha256"] == digest(root / "ui/assets/heya.svg"), \
        "SVG changed; regenerate with python3 scripts/macos-icons.py --generate"
    assert set(manifest["png_sha256"]) == set(SLOTS), "Incomplete icon manifest"
    assert {path.name for path in directory.glob("*.png")} == set(SLOTS), "Incomplete iconset"
    for name, pixels in SLOTS.items():
        path = directory / name
        png = path.read_bytes()
        assert png[:8] == b"\x89PNG\r\n\x1a\n" and png[12:16] == b"IHDR", name
        assert struct.unpack(">II", png[16:24]) == (pixels, pixels), name
        assert digest(path) == manifest["png_sha256"][name], f"Changed icon: {name}"


def generate(root):
    if not shutil.which("rsvg-convert"):
        raise SystemExit("Install librsvg yourself (Arch: sudo pacman -S librsvg), then retry.")
    source = root / "ui/assets/heya.svg"
    directory = root / "packaging/macos/Heya.iconset"
    directory.mkdir(parents=True, exist_ok=True)
    rendered = {}
    for name, pixels in SLOTS.items():
        if pixels not in rendered:
            rendered[pixels] = subprocess.check_output([
                "rsvg-convert", "--width", str(pixels), "--height", str(pixels), str(source)
            ])
        (directory / name).write_bytes(rendered[pixels])
    manifest = {"source_sha256": digest(source),
                "png_sha256": {name: digest(directory / name) for name in SLOTS}}
    (directory.parent / "icons.json").write_text(json.dumps(manifest, indent=2) + "\n")
    check(root)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--generate", action="store_true", help="Regenerate assets after changing the SVG")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    generate(root) if args.generate else check(root)
    print("Heya macOS iconset matches the canonical SVG; all ten sizes verified.")


if __name__ == "__main__":
    main()
