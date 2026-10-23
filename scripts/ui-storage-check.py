#!/usr/bin/env python3
"""First-run/plaintext/failure checks; only run inside ui-offscreen.sh."""
import json
import os
from contextlib import closing
from pathlib import Path
import runpy
import sqlite3
import subprocess
import sys
import time

data = Path(os.environ["XDG_DATA_HOME"])
assert data.parent.name.startswith("heya-offscreen.")
assert not os.environ.get("WAYLAND_DISPLAY")
assert not os.environ.get("DBUS_SESSION_BUS_ADDRESS")
assert not list(data.rglob("*.db"))
env = os.environ.copy()
env["DBUS_SESSION_BUS_ADDRESS"] = f"unix:path={os.environ['XDG_RUNTIME_DIR']}/no-keyring-bus"
profile = data / "heya" / "default-v2"
prefix = Path(sys.argv[2]).resolve()
request_close = runpy.run_path(str(Path(__file__).with_name("ui-close-check.py")))["request_close"]


def check(name, extra=(), ready=None, action=None, offline=True):
    logfile = Path(f"{prefix}-{name}.log")
    with logfile.open("w") as output:
        proc = subprocess.Popen([sys.argv[1], *(["--offline"] if offline else []), "--size", "900x700", *extra],
                                env=env, stdout=output, stderr=output)
        try:
            window = subprocess.check_output(
                ["xdotool", "search", "--sync", "--onlyvisible", "--pid", str(proc.pid), "--name", "^Heya"],
                text=True, timeout=15).splitlines()[0]
            deadline = time.monotonic() + 10
            while ready and not ready(logfile.read_text()):
                assert proc.poll() is None, logfile.read_text()
                assert time.monotonic() < deadline, logfile.read_text()
                time.sleep(0.05)
            time.sleep(1)
            # Conversion/local settings threads must finish instead of parking.
            while True:
                workers = []
                for task in Path(f"/proc/{proc.pid}/task").glob("*/comm"):
                    try:
                        if task.read_text().strip() == "heya-settings":
                            workers.append(task)
                    except FileNotFoundError:
                        pass
                if not workers:
                    break
                assert time.monotonic() < deadline, "storage worker did not finish"
                time.sleep(0.05)
            time.sleep(0.1)
            subprocess.run(["import", "-window", window, f"{prefix}-{name}.png"], check=True)
            if action:
                action(window)
            request_close(window)
            assert proc.wait(timeout=5) == 0, logfile.read_text()
            log = logfile.read_text()
            assert "Heya connection: Connecting" not in log, "offline storage check tried networking"
            assert "shutdown was interrupted" not in log, log
            print(f"{name}: native window, finite workers and normal shutdown passed", flush=True)
        finally:
            if proc.poll() is None:
                proc.terminate()
                try:
                    proc.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    proc.wait()



def choose_plaintext(window):
    # Fixed 900x700 fixture, checked against the captured native layout.
    for x, y in [(620, 380), (480, 447)]:
        subprocess.run(["xdotool", "mousemove", "--window", window, str(x), str(y), "click", "1"], check=True)
    deadline = time.monotonic() + 5
    while not (profile / "storage.json").exists():
        assert time.monotonic() < deadline, "first-run choice was not saved"
        time.sleep(0.05)
    assert json.loads((profile / "storage.json").read_text())["mode"] == "unencrypted"
    time.sleep(0.2)
    subprocess.run(["import", "-window", window, f"{prefix}-chosen-plaintext.png"], check=True)


check("first-run", action=choose_plaintext)
assert not list(data.rglob("*.db")), "first-run choice accessed account storage"
assert (profile / "storage.json").exists()
with closing(sqlite3.connect(profile / "session.db")) as session:
    session.execute("CREATE TABLE synthetic_probe(value TEXT)")
    session.execute("INSERT INTO synthetic_probe VALUES('Synthetic session')")
    session.commit()
check("plaintext", ready=lambda log: "Heya connection: Stopped" in log)
with closing(sqlite3.connect(profile / "archive.db")) as archive:
    assert archive.execute("PRAGMA user_version").fetchone()[0] == 18
    archive.execute("SELECT count(*) FROM messages").fetchone()
before = {name: (profile / name).read_bytes() for name in ("storage.json", "session.db", "archive.db")}
check("locked-key-store", ("--change-storage", "encrypted"))
for name, content in before.items():
    assert (profile / name).read_bytes() == content, "failed conversion replaced active plaintext data"
assert not (profile / "data-a").exists()
assert not (profile / "data-b").exists()
print("Plaintext opened with no key store; failed encryption retained both databases and the manifest.")

# A separate synthetic encrypted profile starts normally before its key store.
# An invalid bus prevents any access to the owner's key store or network session.
encrypted_data = data / "encrypted-wait"
encrypted_profile = encrypted_data / "heya" / "default-v2"
encrypted_profile.mkdir(parents=True)
synthetic_session = encrypted_profile / "session.db"
synthetic_session.write_bytes(b"Synthetic encrypted session")
env["XDG_DATA_HOME"] = str(encrypted_data)
check("waiting-key-store", ready=lambda log: "Heya storage: WaitingForKeyStore" in log, offline=False)
assert synthetic_session.read_bytes() == b"Synthetic encrypted session"
assert not (encrypted_profile / "archive.db").exists()
print("Encrypted automatic startup waits for the key store and closes without a parked worker or data changes.")
