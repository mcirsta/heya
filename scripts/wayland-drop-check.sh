#!/usr/bin/env bash
# Invoked by ui-offscreen.sh, inside its private D-Bus and runtime directory.
set -euo pipefail
heya_binary=$1
heya_root=$2
export HEYA_PRIVATE_DROP_TEST=1
export WAYLAND_DISPLAY=heya-drop-test
export LIBGL_ALWAYS_SOFTWARE=1 SLINT_BACKEND=winit-skia-software
QT_QPA_PLATFORM=offscreen kwin_wayland --virtual --socket "$WAYLAND_DISPLAY" \
    --no-lockscreen --no-global-shortcuts --no-kactivities >"$XDG_RUNTIME_DIR/compositor.log" 2>&1 &
heya_compositor=$!
heya_test=""
trap 'if [[ -n "$heya_test" ]]; then kill "$heya_test" 2>/dev/null || true; wait "$heya_test" 2>/dev/null || true; fi; kill "$heya_compositor" 2>/dev/null || true; wait "$heya_compositor" 2>/dev/null || true' EXIT
for ((heya_attempt=0; heya_attempt<100; heya_attempt++)); do
    if [[ -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" ]]; then break; fi
    sleep 0.1
done
[[ -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" ]] || { cat "$XDG_RUNTIME_DIR/compositor.log" >&2; exit 1; }
for ((heya_attempt=0; heya_attempt<100; heya_attempt++)); do
    if qdbus6 org.kde.KWin /Scripting >"$XDG_RUNTIME_DIR/scripting.txt" 2>/dev/null; then break; fi
    if ! kill -0 "$heya_compositor" 2>/dev/null; then cat "$XDG_RUNTIME_DIR/compositor.log" >&2; exit 1; fi
    sleep 0.1
done
cat >"$XDG_RUNTIME_DIR/place.js" <<'JS'
function place(w) {
    if (w.caption === "Heya drop source") w.frameGeometry = {x:0, y:0, width:250, height:200};
    else if (w.caption === "Heya") w.frameGeometry = {x:300, y:0, width:900, height:700};
    print("heya-test-window", w.caption, w.frameGeometry.x, w.frameGeometry.y, w.frameGeometry.width, w.frameGeometry.height);
}
workspace.windowAdded.connect(place);
workspace.windowList().forEach(place);
JS
heya_script=$(qdbus6 org.kde.KWin /Scripting org.kde.kwin.Scripting.loadScript "$XDG_RUNTIME_DIR/place.js" heya-test) || { cat "$XDG_RUNTIME_DIR/scripting.txt" "$XDG_RUNTIME_DIR/compositor.log" >&2; exit 1; }
qdbus6 org.kde.KWin "/Scripting/Script$heya_script" org.kde.kwin.Script.run
heya_protocol=/usr/share/plasma-wayland-protocols/fake-input.xml
wayland-scanner client-header "$heya_protocol" "$XDG_RUNTIME_DIR/fake-input-client.h"
wayland-scanner private-code "$heya_protocol" "$XDG_RUNTIME_DIR/fake-input.c"
cc -c "$XDG_RUNTIME_DIR/fake-input.c" -o "$XDG_RUNTIME_DIR/fake-input.o" $(pkg-config --cflags wayland-client)
c++ -std=c++17 -O2 -fPIC -Wall -Wextra "$heya_root/scripts/wayland-drop-source.cpp" \
    "$XDG_RUNTIME_DIR/fake-input.o" -I"$XDG_RUNTIME_DIR" \
    $(pkg-config --cflags --libs Qt6Widgets wayland-client) -o "$XDG_RUNTIME_DIR/drop-source"
for heya_mode in drop cancel text; do
    HEYA_TEST_DROP_MODE="$heya_mode" "$heya_binary" --ignored --exact \
        presentation_clipboard::tests::native_wayland_drop --nocapture >"$XDG_RUNTIME_DIR/receiver.log" 2>&1 &
    heya_test=$!
    sleep 3
    if ! QT_QPA_PLATFORM=wayland "$XDG_RUNTIME_DIR/drop-source" "$heya_mode"; then
        cat "$XDG_RUNTIME_DIR/receiver.log" "$XDG_RUNTIME_DIR/compositor.log" >&2
        exit 1
    fi
    if ! wait "$heya_test"; then cat "$XDG_RUNTIME_DIR/receiver.log" >&2; exit 1; fi
    heya_test=""
    cat "$XDG_RUNTIME_DIR/receiver.log"
done
