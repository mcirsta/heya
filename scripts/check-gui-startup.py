#!/usr/bin/env python3
"""Bounded native GUI startup check using synthetic data, never an account.

On Linux run through scripts/ui-offscreen.sh --startup-check. On macOS CI the
disposable runner owns the desktop; never run this on the owner's working desktop.
"""
import argparse
import os
from pathlib import Path
import subprocess
import tempfile


def check(binary, directory, software):
    mode = "software" if software else "default"
    log = directory / f"{mode}.log"
    # Fresh files make a stale successful run unable to mask a failed startup.
    log.unlink(missing_ok=True)
    environment = os.environ.copy()
    environment.pop("SLINT_BACKEND", None)
    command = [str(binary), "--demo", "--startup-check", "--startup-trace", str(log)]
    if software:
        command.append("--software-rendering")
    try:
        result = subprocess.run(command, env=environment, capture_output=True, timeout=45)
    except subprocess.TimeoutExpired as error:
        # subprocess.run kills and reaps the child on timeout.
        (directory / f"{mode}-stderr.log").write_bytes(error.stderr or b"")
        raise AssertionError(f"{mode}: GUI startup timed out; inspect {log}") from error
    (directory / f"{mode}-stderr.log").write_bytes(result.stderr)
    assert result.returncode == 0, f"{mode}: GUI exited {result.returncode}; inspect {directory}"
    text = log.read_text()
    required = ["stage=process_start ", "stage=backend_select ok", "stage=window_create ok",
                "stage=window_show ok", "stage=native_window width=", "stage=first_redraw begin",
                "stage=first_redraw_returned ok", "stage=shutdown_finished ", "stage=process_exit ok"]
    for checkpoint in required:
        assert checkpoint in text, f"{mode}: missing {checkpoint!r}; inspect {log}"
    assert "stage=panic " not in text
    if "os=macos " in text:
        assert text.index("stage=window_show ok") < text.index("stage=presentation_attach begin")
    assert "mode=demo" in text
    assert log.stat().st_size <= 64 * 1024
    print(f"{mode}: native window, first redraw return and full shutdown passed (synthetic data)")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--log-dir", type=Path)
    parser.add_argument("--software-only", action="store_true")
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="heya-gui-startup-") as temporary:
        directory = args.log_dir.resolve() if args.log_dir else Path(temporary)
        directory.mkdir(parents=True, exist_ok=True)
        if not args.software_only:
            check(binary, directory, False)
        check(binary, directory, True)


if __name__ == "__main__":
    main()
