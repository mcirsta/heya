#!/usr/bin/env python3
"""Synthetic decoder stop/parent-pipe-close regression; no account or UI."""
import json
import os
import select
import struct
import subprocess
import sys
import time
from pathlib import Path


def check(binary, clip, close_pipe):
    header = json.dumps({
        "demo": False, "silent": True,
        "settings": {
            "input": {"id": "", "name": ""}, "output": {"id": "", "name": ""},
            "input_volume": 100, "output_volume": 100,
        },
    }).encode()
    with subprocess.Popen([binary, "--heya-video-worker"], stdin=subprocess.PIPE,
                          stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, bufsize=0) as child:
        try:
            # write() on an unbuffered pipe may be partial.
            for part in (struct.pack("<I", len(header)), header, struct.pack("<I", len(clip)), clip):
                data = memoryview(part)
                while data:
                    count = child.stdin.write(data)
                    assert count
                    data = data[count:]
            deadline = time.monotonic() + 10

            def read(count):
                result = bytearray()
                while len(result) < count:
                    remaining = deadline - time.monotonic()
                    assert remaining > 0 and select.select([child.stdout], [], [], remaining)[0], "decoder timeout"
                    chunk = os.read(child.stdout.fileno(), min(count - len(result), 65536))
                    assert chunk, "unexpected decoder exit"
                    result.extend(chunk)
                return result

            assert read(13) == b"HEYA-VIDEO/1\n"
            stopped = False
            while True:
                packet = read(1)[0]
                if packet == 0:
                    size = struct.unpack("<I", read(4))[0]
                    assert size <= 4096
                    assert json.loads(read(size))["error"] is None
                elif packet == 1:
                    width, height = struct.unpack("<II", read(8))
                    assert 0 < width <= 960 and 0 < height <= 960
                    read(width * height * 4)
                    if not stopped:
                        if close_pipe:
                            child.stdin.close()
                        else:
                            child.stdin.write(b'{"playing":false,"muted":false,"seek":null,"stop":true}\n')
                        deadline = time.monotonic() + 3
                        stopped = True
                else:
                    assert packet == 2 and stopped
                    break
            assert child.wait(timeout=3) == 0
        finally:
            if child.poll() is None:
                child.kill()
                child.wait()


if __name__ == "__main__":
    synthetic = Path(sys.argv[2]).read_bytes()
    for parent_closes in (False, True):
        check(sys.argv[1], synthetic, parent_closes)
    print("Native decoder: explicit stop and closed parent pipe both released/reaped the playing process.")
