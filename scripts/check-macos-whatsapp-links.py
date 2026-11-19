#!/usr/bin/env python3
"""Check Launch Services delivery to a bundled Heya in a disposable macOS session."""

import argparse
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time


def bundle_processes(binary):
    output = subprocess.check_output(
        ["/bin/ps", "-axww", "-o", "pid=,command="], text=True, timeout=5
    )
    pids = set()
    for line in output.splitlines():
        fields = line.split(maxsplit=1)
        if len(fields) == 2:
            pid, command = fields
            if command == str(binary) or command.startswith(f"{binary} "):
                pids.add(int(pid))
    return pids


def open_link(bundle, url, trace):
    # The URL goes to Launch Services. Only the demo and trace flags go to argv.
    subprocess.run(
        ["/usr/bin/open", "-a", str(bundle), url,
         "--args", "--demo", "--startup-trace", str(trace)],
        check=True, timeout=20,
    )


def wait_for(trace, *markers):
    deadline = time.monotonic() + 45
    while time.monotonic() < deadline:
        if trace.exists():
            text = trace.read_text()
            if all(marker in text for marker in markers):
                return text
            if "stage=process_exit " in text or "stage=panic " in text:
                break
        time.sleep(0.1)
    raise AssertionError(f"Missing {markers!r}; inspect {trace}")


def stop_bundle(binary):
    for pid in bundle_processes(binary):
        try:
            os.kill(pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        if not bundle_processes(binary):
            return
        time.sleep(0.1)
    for pid in bundle_processes(binary):
        try:
            os.kill(pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    raise AssertionError("The test bundle did not exit after SIGTERM")


def check(bundle, directory):
    binary = bundle / "Contents/MacOS/heya"
    assert binary.is_file(), f"Missing bundled executable at {binary}"
    live_trace = directory / "cold-and-warm.log"
    unexpected_trace = directory / "unexpected-warm-launch.log"
    assert not bundle_processes(binary), "Close the test bundle before starting the check"
    live_trace.unlink(missing_ok=True)
    unexpected_trace.unlink(missing_ok=True)
    try:
        open_link(bundle, "whatsapp://send?phone=15550101010", live_trace)
        started = wait_for(live_trace, "stage=first_redraw_returned ok\n",
                           "stage=whatsapp_link preview\n")
        match = re.search(r"stage=process_start .* pid=(\d+) mode=demo ", started)
        assert match, f"Missing synthetic process identity; inspect {live_trace}"
        pid = int(match[1])
        assert bundle_processes(binary) == {pid}, "Expected one cold Heya launch"
        print("Cold URL reached the preview in the launched demo.", flush=True)

        # If Launch Services starts another process, these args keep it in demo
        # and give it a separate log. The warm marker must be in the first log.
        open_link(bundle, "whatsapp://send?phone=15550101011&text=Caf%C3%A9%0AHello",
                  unexpected_trace)
        wait_for(live_trace, "stage=whatsapp_link preview_with_draft\n")
        assert bundle_processes(binary) == {pid}, "The warm URL must reuse the cold process"
        assert not unexpected_trace.exists(), "The warm URL started another Heya process"
        print("Warm URL reached the preview with a draft in the same demo process.", flush=True)
    finally:
        stop_bundle(binary)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("bundle", type=Path)
    parser.add_argument("--log-dir", required=True, type=Path)
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("This check needs a macOS desktop session.")
    directory = args.log_dir.resolve()
    directory.mkdir(parents=True, exist_ok=True)
    check(args.bundle.resolve(strict=True), directory)


if __name__ == "__main__":
    main()
