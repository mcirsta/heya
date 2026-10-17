#!/usr/bin/env python3
"""Real WM_DELETE_WINDOW and process-exit check; run via ui-offscreen.sh only."""
import os
import ctypes
import ctypes.util
from pathlib import Path
import subprocess
import sys
import tempfile
import time


def request_close(window):
    # Xvfb has no window manager to handle xdotool windowquit's _NET_CLOSE_WINDOW
    # request. Deliver WM_DELETE_WINDOW directly to the client instead.
    class Data(ctypes.Union):
        _fields_ = [("bytes", ctypes.c_char * 20), ("longs", ctypes.c_long * 5)]

    class ClientMessage(ctypes.Structure):
        _fields_ = [
            ("type", ctypes.c_int), ("serial", ctypes.c_ulong),
            ("send_event", ctypes.c_int), ("display", ctypes.c_void_p),
            ("window", ctypes.c_ulong), ("message_type", ctypes.c_ulong),
            ("format", ctypes.c_int), ("data", Data),
        ]

    class Event(ctypes.Union):
        _fields_ = [("client", ClientMessage), ("pad", ctypes.c_long * 24)]

    x11 = ctypes.CDLL(ctypes.util.find_library("X11"))
    x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
    x11.XOpenDisplay.restype = ctypes.c_void_p
    x11.XInternAtom.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int]
    x11.XInternAtom.restype = ctypes.c_ulong
    x11.XSendEvent.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_long, ctypes.POINTER(Event)]
    x11.XSync.argtypes = [ctypes.c_void_p, ctypes.c_int]
    x11.XCloseDisplay.argtypes = [ctypes.c_void_p]
    display = x11.XOpenDisplay(None)
    if not display:
        raise RuntimeError("Cannot open the private test display")
    try:
        event = Event()
        event.client.type = 33  # ClientMessage
        event.client.display = display
        event.client.window = int(window)
        event.client.message_type = x11.XInternAtom(display, b"WM_PROTOCOLS", False)
        event.client.format = 32
        event.client.data.longs[0] = x11.XInternAtom(display, b"WM_DELETE_WINDOW", False)
        if not x11.XSendEvent(display, int(window), False, 0, ctypes.byref(event)):
            raise RuntimeError("Cannot send the native close request")
        x11.XSync(display, False)
    finally:
        x11.XCloseDisplay(display)

def main():
    # The no-session startup case exercises the real live service wiring, but must
    # never see the owner's account, keyring or desktop. The wrapper isolates all
    # of these before starting this script.
    data = Path(os.environ.get("XDG_DATA_HOME", "/"))
    assert data.parent.name.startswith("heya-offscreen.")
    assert not os.environ.get("DBUS_SESSION_BUS_ADDRESS")
    assert not os.environ.get("WAYLAND_DISPLAY")
    assert not list(data.rglob("*.db"))

    for mode, delay in [("demo", 0), ("startup", 0), ("demo", 0.5), ("startup", 0.5)]:
        args = [sys.argv[1], "--size", "800x600"]
        if mode == "demo":
            args += ["--demo", "--demo-chat", "10"]
        with tempfile.TemporaryFile(mode="w+") as log:
            proc = subprocess.Popen(args, stdout=log, stderr=log)
            try:
                window = subprocess.check_output(
                    ["xdotool", "search", "--sync", "--onlyvisible", "--pid", str(proc.pid), "--name", "^Heya"],
                    timeout=15, text=True,
                ).splitlines()[0]
                time.sleep(delay)
                # Startup, including opening the audio demo, must not reserve a
                # playback thread. Only an explicit Play with ready bytes starts it.
                audio_threads = []
                for task in Path(f"/proc/{proc.pid}/task").glob("*/comm"):
                    try:
                        if task.read_text().strip() in {"heya-audio", "heya-prepare", "heya-upload", "heya-upload-io"}:
                            audio_threads.append(task.parent.name)
                    except FileNotFoundError:
                        pass  # Unrelated short-lived worker exited during enumeration.
                assert not audio_threads, f"idle startup created media threads: {audio_threads}"
                started = time.monotonic()
                request_close(window)
                code = proc.wait(timeout=3)
                elapsed = (time.monotonic() - started) * 1000
                log.seek(0)
                output = log.read()
                assert code == 0, output
                assert "Heya shutdown: finished" in output, output
                assert "shutdown was interrupted" not in output, output
                assert "did not finish" not in output, output
                print(f"{mode}: no idle media thread; close/reopen exited in {elapsed:.0f} ms", flush=True)
            except Exception:
                log.seek(0)
                print(log.read(), file=sys.stderr, flush=True)
                raise
            finally:
                if proc.poll() is None:
                    proc.terminate()
                    try:
                        proc.wait(timeout=3)
                    except subprocess.TimeoutExpired:
                        proc.kill()
                        proc.wait()
    assert not list(data.rglob("*.db")), "startup check must not create an account"


if __name__ == "__main__":
    main()
