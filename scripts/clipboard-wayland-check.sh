#!/usr/bin/env bash
# Invoked only by ui-offscreen.sh with a private runtime directory and D-Bus.
set -euo pipefail
export QT_QPA_PLATFORM=offscreen LIBGL_ALWAYS_SOFTWARE=1
export WAYLAND_DISPLAY=heya-clipboard-test
kwin_wayland --virtual --socket "$WAYLAND_DISPLAY" --no-lockscreen \
    --no-global-shortcuts --no-kactivities >"$XDG_RUNTIME_DIR/compositor.log" 2>&1 &
heya_compositor=$!
trap 'kill "$heya_compositor" 2>/dev/null || true; wait "$heya_compositor" 2>/dev/null || true' EXIT
for ((heya_attempt=0; heya_attempt<100; heya_attempt++)); do
    if [[ -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" ]]; then break; fi
    if ! kill -0 "$heya_compositor" 2>/dev/null; then
        cat "$XDG_RUNTIME_DIR/compositor.log" >&2
        exit 1
    fi
    sleep 0.1
done
[[ -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" ]] || { cat "$XDG_RUNTIME_DIR/compositor.log" >&2; exit 1; }
HEYA_PRIVATE_CLIPBOARD_TEST=1 HEYA_TEST_WAYLAND_CLIPBOARD=1 "$1" \
    --ignored --exact clipboard::tests::native_clipboard_roundtrip --nocapture
