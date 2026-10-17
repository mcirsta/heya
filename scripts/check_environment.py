#!/usr/bin/env python3
"""Inspect Heya's proposed Linux build prerequisites without changing the host."""

import argparse
import json
import platform
import re
import shutil
import subprocess
import sys


def inspect_command(label, command, required=True, minimum=None):
    executable = shutil.which(command[0])
    if executable is None:
        return {"check": label, "status": "FAIL" if required else "WARN",
                "detail": f"{command[0]} not found"}
    try:
        result = subprocess.run(command, capture_output=True, text=True, timeout=15)
    except (OSError, subprocess.TimeoutExpired) as error:
        return {"check": label, "status": "FAIL" if required else "WARN",
                "detail": str(error)}
    output = (result.stdout or result.stderr).strip()
    first_line = output.splitlines()[0] if output else f"exit {result.returncode}"
    passed = result.returncode == 0
    if passed and minimum is not None:
        match = re.search(r"(\d+)\.(\d+)(?:\.(\d+))?", output)
        version = tuple(int(part or 0) for part in match.groups()) if match else None
        passed = version is not None and version >= minimum
        if not passed:
            first_line += " (requires " + ".".join(map(str, minimum)) + "+)"
    return {"check": label, "status": "OK" if passed else ("FAIL" if required else "WARN"),
            "detail": first_line}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--media", action="store_true", help="require Linux media spike prerequisites")
    parser.add_argument("--json", action="store_true", help="print a JSON report")
    args = parser.parse_args()
    checks = [
        {"check": "Python", "status": "OK" if sys.version_info >= (3, 11) else "FAIL",
         "detail": platform.python_version() + " (requires 3.11+)"},
        inspect_command("Rust candidate", ["rustc", "--version"], minimum=(1, 99, 0)),
        inspect_command("Cargo", ["cargo", "--version"]),
        inspect_command("rustfmt", ["cargo", "fmt", "--version"]),
        inspect_command("clippy", ["cargo", "clippy", "--version"]),
        inspect_command("Git", ["git", "--version"]),
    ]
    if platform.system() == "Linux":
        checks.extend([
            inspect_command("C compiler", ["cc", "--version"]),
            inspect_command("pkg-config", ["pkg-config", "--version"]),
            inspect_command("CMake", ["cmake", "--version"], required=False),
            inspect_command("Ninja", ["ninja", "--version"], required=False),
            inspect_command("Perl for vendored OpenSSL", ["perl", "-e", "print $^V" ]),
            inspect_command("Make for vendored OpenSSL", ["make", "--version"]),
            inspect_command("PDF preview renderer", ["/usr/bin/pdftoppm", "-v"], required=args.media),
            inspect_command("PDF preview sandbox", ["/usr/bin/bwrap", "--version"], required=args.media),
            inspect_command("PDF preview resource limits", ["/usr/bin/prlimit", "--version"], required=args.media),
        ])
        for library in ("wayland-client", "xkbcommon"):
            checks.append(inspect_command(library, ["pkg-config", "--modversion", library]))
        for library in ("gstreamer-1.0", "gstreamer-app-1.0", "gstreamer-video-1.0"):
            checks.append(inspect_command(library, ["pkg-config", "--modversion", library],
                                          required=args.media, minimum=(1, 20, 0)))
        for plugin in ("playbin", "appsink", "videoconvert", "audioconvert", "autoaudiosink",
                       "qtdemux", "avdec_h264", "avdec_aac", "opusdec"):
            checks.append(inspect_command("GStreamer " + plugin,
                                          ["gst-inspect-1.0", plugin], required=args.media))
    else:
        checks.append({"check": "Platform", "status": "FAIL" if args.media else "WARN",
                       "detail": "Native checks currently cover Linux; Windows/macOS are roadmap work"})
    failures = sum(check["status"] == "FAIL" for check in checks)
    if args.json:
        print(json.dumps({"platform": platform.system(), "checks": checks,
                          "failures": failures, "application_build_verified": False}, indent=2))
    else:
        for check in checks:
            print(f"{check['status']:4} {check['check']}: {check['detail']}")
        print(f"\n{failures} required checks failed. This is an environment check, not a Heya build.")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
