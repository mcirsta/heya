#!/usr/bin/env bash
# Synthetic UI checks on a private virtual display; never the owner's desktop.
set -euo pipefail

usage() {
    echo "Usage: $0 {--screenshot|--framebuffer} /tmp/heya.png [--size WIDTHxHEIGHT] [--light|--dark] [--demo-chat ID] [--click XxY (framebuffer only)] [--hover XxY (framebuffer only)] | --ui-benchmark | --call-audio-check | --video-check | --wheel-check | --close-check | --startup-check | --clipboard-check | --wayland-clipboard-check | --wayland-drop-check | --clipboard-ui-check OUTPUT_PREFIX | --storage-check OUTPUT_PREFIX" >&2
    exit 2
}

heya_args=("${1:-}")
heya_click=""
heya_hover=""
case "${1:-}" in
    --screenshot|--framebuffer)
        [[ $# -ge 2 && -n "$2" ]] || usage
        heya_args+=("$2")
        shift 2
        while [[ $# -gt 0 ]]; do
            case "$1" in
                --size)
                    [[ $# -ge 2 && "$2" =~ ^[0-9]+x[0-9]+$ ]] || usage
                    heya_args+=("$1" "$2")
                    shift 2
                    ;;
                --demo-chat)
                    [[ $# -ge 2 && "$2" =~ ^([1-9]|1[0-6])$ ]] || usage
                    heya_args+=("$1" "$2")
                    shift 2
                    ;;
                --light|--dark) heya_args+=("$1"); shift ;;
                --click)
                    [[ "${heya_args[0]}" == --framebuffer && $# -ge 2 && "$2" =~ ^[0-9]+x[0-9]+$ ]] || usage
                    heya_click=$2
                    shift 2
                    ;;
                --hover)
                    [[ "${heya_args[0]}" == --framebuffer && $# -ge 2 && "$2" =~ ^[0-9]+x[0-9]+$ ]] || usage
                    heya_hover=$2
                    shift 2
                    ;;
                *) usage ;;
            esac
        done
        ;;
    --clipboard-ui-check|--storage-check)
        [[ $# == 2 && -n "$2" ]] || usage
        heya_args+=("$2")
        ;;
    --call-audio-check|--video-check|--ui-benchmark|--wheel-check|--close-check|--startup-check|--clipboard-check|--wayland-clipboard-check|--wayland-drop-check)
        [[ $# == 1 ]] || usage
        ;;
    *)
        usage
        ;;
esac

heya_display_tools=(Xvfb xvfb-run xauth)
if [[ "${heya_args[0]}" == --wayland-drop-check ]]; then
    heya_display_tools=(kwin_wayland dbus-run-session qdbus6 wayland-scanner pkg-config cc c++)
elif [[ "${heya_args[0]}" == --wayland-clipboard-check ]]; then
    heya_display_tools=(kwin_wayland dbus-run-session wl-copy wl-paste)
fi
for program in "${heya_display_tools[@]}"; do
    if ! command -v "$program" >/dev/null; then
        echo "Missing offscreen test tool: $program" >&2
        exit 1
    fi
done

heya_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
heya_binary=${HEYA_TEST_BINARY:-"$heya_root/target/release/heya"}
if [[ ! -x "$heya_binary" ]]; then
    echo "Build first: cargo build --release --locked" >&2
    exit 1
fi
heya_renderer=${HEYA_TEST_RENDERER:-winit-skia-software}
case "$heya_renderer" in
    winit-skia-software|winit-skia-opengl|winit-skia) ;;
    *) echo "Unsupported HEYA_TEST_RENDERER" >&2; exit 2 ;;
esac

heya_command=("$heya_binary" --demo "${heya_args[@]}")
if [[ "${heya_args[0]}" == --call-audio-check ]]; then
    heya_command=(env HEYA_TEST_CALL_GSTREAMER=1 "$heya_binary" --ignored --exact call_audio::tests::native_gstreamer_duplex_audio --nocapture)
fi
if [[ "${heya_args[0]}" == --video-check ]]; then
    heya_command=(bash "$heya_root/scripts/video-check.sh" "$heya_binary" "$heya_root/target/release/heya")
fi
if [[ "${heya_args[0]}" == --clipboard-ui-check ]]; then
    for program in xclip xdotool import identify cc; do
        command -v "$program" >/dev/null || { echo "Clipboard UI check needs $program." >&2; exit 1; }
    done
    heya_command=(bash "$heya_root/scripts/ui-clipboard-check.sh" "$heya_binary" "${heya_args[1]}" "$heya_root")
fi
if [[ "${heya_args[0]}" == --storage-check ]]; then
    for program in python3 xdotool import; do
        command -v "$program" >/dev/null || { echo "Storage check needs $program." >&2; exit 1; }
    done
    heya_command=(python3 "$heya_root/scripts/ui-storage-check.py" "$heya_binary" "${heya_args[1]}")
fi
if [[ "${heya_args[0]}" == --clipboard-check ]]; then
    command -v xclip >/dev/null || { echo "Install xclip with pacman." >&2; exit 1; }
    # Pass the Cargo test executable explicitly; never use the owner's clipboard.
    heya_command=(env HEYA_PRIVATE_CLIPBOARD_TEST=1 "$heya_binary" --ignored --exact clipboard::tests::native_clipboard_roundtrip --nocapture)
fi
if [[ "${heya_args[0]}" == --startup-check ]]; then
    heya_command=(python3 "$heya_root/scripts/check-gui-startup.py" "$heya_binary" --software-only)
fi
if [[ "${heya_args[0]}" == --close-check ]]; then
    for program in python3 xdotool; do
        command -v "$program" >/dev/null || { echo "Close check needs $program." >&2; exit 1; }
    done
    heya_command=(python3 "$heya_root/scripts/ui-close-check.py" "$heya_binary")
fi
if [[ "${heya_args[0]}" == --wheel-check ]]; then
    command -v xdotool >/dev/null || { echo 'Install xdotool with pacman.' >&2; exit 1; }
    heya_command=(bash -c '
        set -euo pipefail
        HEYA_TEST_WHEEL_TRACE=1 "$1" --demo --demo-chat 9 --size 1200x900 &
        heya_pid=$!
        trap '\''kill "$heya_pid" 2>/dev/null || true; wait "$heya_pid" 2>/dev/null || true'\'' EXIT
        heya_window=$(timeout 15s xdotool search --sync --onlyvisible --name "^Heya" | head -n 1)
        sleep 2
        xdotool mousemove --window "$heya_window" 800 350
        # winit XInput2 reports both synthetic button press and release as
        # wheel events. Separate them to measure one line event at a time.
        echo "Native wheel: up one line event"
        xdotool mousedown 4
        sleep 0.4
        echo "Native wheel: up second line event (release)"
        xdotool mouseup 4
        sleep 0.4
        echo "Native wheel: down one line event"
        xdotool mousedown 5
        sleep 0.4
        echo "Native wheel: down second line event (release)"
        xdotool mouseup 5
        sleep 0.4
        echo "Native wheel: up three notches in a burst"
        xdotool click --repeat 3 --delay 35 4
        sleep 0.4
        echo "Native wheel: reverse down three notches"
        xdotool click --repeat 3 --delay 35 5
        sleep 0.4
    ' bash "$heya_binary")
fi
if [[ "${heya_args[0]}" == --framebuffer ]]; then
    for program in xdotool import; do
        if ! command -v "$program" >/dev/null; then
            echo "Framebuffer capture needs $program (pacman: xdotool imagemagick)." >&2
            exit 1
        fi
    done
    # Capture the actual synthetic X11 window. Slint's take_snapshot re-renders
    # into a CPU canvas, which can omit cached GPU textures. This command runs
    # only inside the private X server below; it cannot see desktop windows.
    heya_command=(bash -c '
        set -euo pipefail
        heya_output=$1
        heya_click=$2
        heya_hover=$3
        shift 3
        "$@" &
        heya_pid=$!
        trap '\''kill "$heya_pid" 2>/dev/null || true; wait "$heya_pid" 2>/dev/null || true'\'' EXIT
        heya_window=$(timeout 15s xdotool search --sync --onlyvisible --name "^Heya" | head -n 1)
        # Mapping precedes first-frame layout/font work; let the static demo settle.
        sleep 2
        if [[ -n "$heya_click" ]]; then
            xdotool mousemove --window "$heya_window" "${heya_click%x*}" "${heya_click#*x}" click 1
            # Cold plugin initialization can outlast one frame. Capture the
            # requested synthetic state without touching the working desktop.
            heya_wait=${HEYA_TEST_CLICK_WAIT:-1}
            [[ "$heya_wait" =~ ^[1-5]$ ]] || exit 2
            sleep "$heya_wait"
        fi
        if [[ -n "$heya_hover" ]]; then
            xdotool mousemove --window "$heya_window" "${heya_hover%x*}" "${heya_hover#*x}"
            sleep 1
        fi
        import -window "$heya_window" "$heya_output"
        echo "Saved synthetic X11 framebuffer to $heya_output"
    ' bash "${heya_args[1]}" "$heya_click" "$heya_hover" "$heya_binary" --demo "${heya_args[@]:2}")
fi

# Optional genuine stationary pointer over the scrolling transcript. This uses
# the same private display as the benchmark; it cannot move the desktop pointer.
if [[ "${heya_args[0]}" == --ui-benchmark && "${HEYA_BENCHMARK_HOVER:-0}" == 1 ]]; then
    command -v xdotool >/dev/null || { echo 'Install xdotool with pacman.' >&2; exit 1; }
    heya_command=(bash -c '
        set -euo pipefail
        "$1" --demo --ui-benchmark &
        heya_pid=$!
        trap '\''kill "$heya_pid" 2>/dev/null || true; wait "$heya_pid" 2>/dev/null || true'\'' EXIT
        heya_window=$(timeout 15s xdotool search --sync --onlyvisible --name "^Heya" | head -n 1)
        xdotool mousemove --window "$heya_window" 550 350
        wait "$heya_pid"
    ' bash "$heya_binary")
fi

if [[ "${heya_args[0]}" == --ui-benchmark ]]; then
    echo "Offscreen smoke test: no desktop compositor; maximize/restore is not validated." >&2
    if [[ "$heya_renderer" == winit-skia-software ]]; then
        echo "Skia CPU render/frame counters are unavailable (printed zeros are not measured frames). Use layout/snapshot timings only." >&2
    fi
fi

heya_test_dir=$(mktemp -d "${TMPDIR:-/tmp}/heya-offscreen.XXXXXXXX")
trap 'rm -rf -- "$heya_test_dir"' EXIT
mkdir -m 700 "$heya_test_dir/runtime" "$heya_test_dir/config" "$heya_test_dir/data" "$heya_test_dir/cache"

if [[ "${heya_args[0]}" == --wayland-clipboard-check || "${heya_args[0]}" == --wayland-drop-check ]]; then
    heya_wayland_script=clipboard-wayland-check.sh
    if [[ "${heya_args[0]}" == --wayland-drop-check ]]; then heya_wayland_script=wayland-drop-check.sh; fi
    # No desktop-service autostart: a headless compositor needs neither portals
    # nor a document-portal FUSE mount that can outlive this isolated test bus.
    cat >"$heya_test_dir/bus.conf" <<'XML'
<busconfig>
  <type>session</type><listen>unix:tmpdir=/tmp</listen><auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/><allow own="*"/>
  </policy>
</busconfig>
XML
    timeout --signal=TERM --kill-after=5s 60s env -u DISPLAY -u WAYLAND_DISPLAY -u WAYLAND_SOCKET -u XAUTHORITY \
        -u DBUS_SESSION_BUS_ADDRESS -u SESSION_MANAGER -u AT_SPI_BUS_ADDRESS \
        XDG_RUNTIME_DIR="$heya_test_dir/runtime" XDG_CONFIG_HOME="$heya_test_dir/config" \
        XDG_DATA_HOME="$heya_test_dir/data" XDG_CACHE_HOME="$heya_test_dir/cache" \
        dbus-run-session --config-file="$heya_test_dir/bus.conf" -- \
        bash "$heya_root/scripts/$heya_wayland_script" "$heya_binary" "$heya_root"
    exit $?
fi

# Removing both Wayland entry points and using a private runtime directory
# prevents an X11 failure from selecting the owner's real Wayland compositor.
# xvfb-run supplies its own DISPLAY and XAUTHORITY and cleans up on completion.
heya_timeout=90s
if [[ "${HEYA_BENCHMARK_ARCHIVE:-0}" == 1 ]]; then heya_timeout=180s; fi
timeout --signal=TERM --kill-after=5s "$heya_timeout" env -u DISPLAY -u WAYLAND_DISPLAY -u WAYLAND_SOCKET -u XAUTHORITY \
    -u DBUS_SESSION_BUS_ADDRESS -u SESSION_MANAGER -u AT_SPI_BUS_ADDRESS \
    NO_AT_BRIDGE=1 \
    XDG_RUNTIME_DIR="$heya_test_dir/runtime" \
    XDG_CONFIG_HOME="$heya_test_dir/config" \
    XDG_DATA_HOME="$heya_test_dir/data" \
    XDG_CACHE_HOME="$heya_test_dir/cache" \
    SLINT_BACKEND="$heya_renderer" \
    xvfb-run -a -s "-screen 0 3840x2160x24 -nolisten tcp" \
    "${heya_command[@]}"
