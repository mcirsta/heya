#!/usr/bin/env bash
# Launched only by ui-offscreen.sh, with a private display and synthetic files.
set -euo pipefail
heya_binary=$1
heya_output=$2
heya_root=$3
heya_fixture=$(mktemp -d /tmp/heya-clipboard-ui.XXXXXXXX)
heya_pid=""
trap 'if [[ -n "$heya_pid" ]]; then kill "$heya_pid" 2>/dev/null || true; wait "$heya_pid" 2>/dev/null || true; fi; rm -rf -- "$heya_fixture"' EXIT
cp "$heya_root/tests/fixtures/preview-strip.png" "$heya_fixture/Sample 你好.png"
printf 'Synthetic document\n' > "$heya_fixture/Notes.txt"
cc -O2 -Wall -Wextra "$heya_root/scripts/ui-drop-source.c" -lX11 -o "$heya_fixture/drop-source"
"$heya_binary" --demo --demo-chat 9 --size 1200x900 > "$heya_fixture/app.log" 2>&1 &
heya_pid=$!
heya_window=$(timeout 15s xdotool search --sync --onlyvisible --name '^Heya' | head -n1)
sleep 2
xclip -selection clipboard -in -target image/png "$heya_fixture/Sample 你好.png"
xdotool windowfocus "$heya_window" mousemove --window "$heya_window" 700 842 click 1 key ctrl+v
sleep 2
import -window "$heya_window" "${heya_output}-paste.png"
xdotool key Escape
sleep 0.5
heya_uris=$(python3 - "$heya_fixture" <<'PY'
from pathlib import Path
import sys
p=Path(sys.argv[1])
print('\r\n'.join((p/n).as_uri() for n in ('Sample 你好.png','Notes.txt')),end='')
PY
)
"$heya_fixture/drop-source" "$heya_window" "$heya_uris"
sleep 2
import -window "$heya_window" "${heya_output}-drop.png"
xdotool key Escape
sleep 0.5
xdotool mousemove --window "$heya_window" 500 100 click 1
sleep 1
# The photo viewer's native shortcut exercises the UI callback and finite worker.
if [[ "${HEYA_TEST_COPY_BUTTON:-0}" == 1 ]]; then
    xdotool mousemove --window "$heya_window" 1064 28 click 1
else
    xdotool key ctrl+c
fi
heya_dimensions=""
for ((heya_attempt=0; heya_attempt<50; heya_attempt++)); do
    timeout 2s xclip -selection clipboard -out -target image/png > "$heya_fixture/copied.png" 2>/dev/null || true
    heya_dimensions=$(identify -quiet -format '%w %h' "$heya_fixture/copied.png" 2>/dev/null || true)
    if [[ "$heya_dimensions" == '1024 768' ]]; then break; fi
    sleep 0.1
done
[[ "$heya_dimensions" == '1024 768' ]] || { echo "Wrong copied image dimensions: $heya_dimensions" >&2; exit 1; }
import -window "$heya_window" "${heya_output}-copy.png"
echo "Native viewer Copy image: full-resolution 1024x768 PNG available to another process"
echo "Synthetic native paste/drop captures: ${heya_output}-{paste,drop}.png"
python3 -B - "$heya_root" "$heya_window" "$heya_pid" <<'PY'
import importlib.util, sys, time
from pathlib import Path
spec = importlib.util.spec_from_file_location('close_check', Path(sys.argv[1])/'scripts/ui-close-check.py')
close_check = importlib.util.module_from_spec(spec)
spec.loader.exec_module(close_check)
process = Path('/proc')/sys.argv[3]
for task in (process/'task').glob('*/comm'):
    try:
        assert task.read_text().strip() not in {'heya-prepare','heya-upload','heya-upload-io'}, 'idle transfer worker survived'
    except FileNotFoundError:
        pass
close_check.request_close(sys.argv[2])
started = time.monotonic()
while process.exists():
    try:
        if (process/'stat').read_text().split(') ',1)[1].startswith('Z '): break
    except FileNotFoundError:
        break
    assert time.monotonic()-started < 3, 'clipboard owner prevented normal shutdown'
    time.sleep(0.01)
print('No idle preparation/upload worker; normal close after Copy image completed')
PY
wait "$heya_pid"
heya_pid=""
