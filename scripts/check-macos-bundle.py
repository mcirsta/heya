#!/usr/bin/env python3
"""Synthetic media check of a packaged app; never opens an account or microphone."""
import argparse
import json
import pathlib
import signal
import struct
import subprocess
import threading
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('bundle', type=pathlib.Path)
parser.add_argument('--native-output', action='store_true',
                    help='Exercise output hardware at zero app volume instead of a silent clock')
args = parser.parse_args()
binary = args.bundle.resolve() / 'Contents/MacOS/heya'
settings = json.dumps({'input': {'id': '', 'name': ''}, 'output': {'id': '', 'name': ''},
                       'input_volume': 100, 'output_volume': 0}).encode()
magic = b'HEYA-VOICE/1\n'


def timeout(_signal, _frame):
    raise TimeoutError('Packaged media check exceeded 30 seconds')


signal.signal(signal.SIGALRM, timeout)
signal.alarm(30)


def start(mode):
    child = subprocess.Popen([str(binary), '--heya-media-worker', mode], stdin=subprocess.PIPE,
                             stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    try:
        assert child.stdout.read(len(magic)) == magic
        child.stdin.write(struct.pack('<I', len(settings)) + settings)
        child.stdin.flush()
        return child
    except BaseException:
        child.kill()
        child.wait()
        raise


def finish(child):
    if child.poll() is None:
        child.kill()
    child.wait(timeout=3)
    child.stdin.close()
    child.stdout.close()


def result(child):
    while True:
        tag = child.stdout.read(1)
        if tag == b'\0':
            assert len(child.stdout.read(72)) == 72
        elif tag == b'\1':
            seconds = struct.unpack('<I', child.stdout.read(4))[0]
            waveform = child.stdout.read(64)
            length = struct.unpack('<I', child.stdout.read(4))[0]
            assert 0 < length <= 20 * 1024 * 1024
            audio = child.stdout.read(length)
            assert seconds >= 1 and len(audio) == length and audio.startswith(b'OggS')
            assert len(waveform) == 64 and max(waveform) > 0
            return audio
        elif tag == b'\3':
            return None
        else:
            raise AssertionError(('Unexpected media result', tag, child.stdout.read(1)))


child = start('record-synthetic')
try:
    def controls():
        for delay, command in [(0.5, 1), (0.2, 0), (0.5, 2)]:
            time.sleep(delay)
            child.stdin.write(bytes([command]))
            child.stdin.flush()
    control_thread = threading.Thread(target=controls, daemon=True)
    control_thread.start()
    audio = result(child)
    assert child.wait(timeout=3) == 0
    control_thread.join(timeout=1)
finally:
    finish(child)
fixture = pathlib.Path(__file__).resolve().parents[1] / 'docs/upstream/macos-opus-prepare-repro/tone.ogg'
for clip in [audio, fixture.read_bytes()]:
    child = start('preview' if args.native_output else 'preview-silent')
    try:
        child.stdin.write(struct.pack('<I', len(clip)) + clip)
        child.stdin.flush()
        assert result(child) is None
        assert child.wait(timeout=3) == 0
    finally:
        finish(child)
signal.alarm(0)
print('Packaged synthetic capture, pause/resume, Opus finalization and preview passed.')
