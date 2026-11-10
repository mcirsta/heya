# Main-window restoration

Normal Heya runs remember the main window's normal client size, maximized state
and fullscreen state. Maximizing or entering fullscreen does not replace the
normal size. Closing while minimized reopens visibly in the preceding mode.
`--size WIDTHxHEIGHT` overrides restoration for that launch; closing can save the
resulting dimensions. Demo and screenshot/benchmark runs leave the owner's state
untouched.

## Platform services

- **macOS:** AppKit `saveFrameUsingName:` / `setFrameUsingName:` store and restore
  the normal frame, including placement, in native application user defaults.
  Two mode bits in the same defaults domain preserve Zoom and Full Screen.
  Saving only the normal frame avoids replacing it with the expanded frame.
  The bridge uses the borrowed NSView on the main thread. No JSON geometry file
  or Linux services are used on macOS.
- **Windows:** `GetWindowPlacement` / `SetWindowPlacement` retain the native
  normal workspace rectangle and restore it with the appropriate show mode.
  Windows adjusts completely offscreen placement after display changes. The
  application still needs to persist the returned data across process exits.
- **Linux (X11 and Wayland):** Slint/winit logical dimensions and mode flags are
  retained. The desktop chooses placement; Wayland does not expose global
  positioning to applications. Restored normal dimensions are fitted to the
  current display, respecting Heya's minimum size.

AppKit's [frame restoration API](https://developer.apple.com/documentation/appkit/nswindow/setframeusingname(_:)?language=objc)
and Microsoft's [window placement API](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowplacement)
provide the native coordinate handling.

On Linux/Windows, `window-state.json` sits beside `settings.json` in the platform
configuration directory (`~/.config/heya/` on Linux). It contains no account data.
A bounded read runs during existing background startup preparation. Missing state
uses defaults; malformed, unsupported or invalid state also falls back to defaults
with a fixed diagnostic. A private temporary file is atomically replaced once
during orderly shutdown, on the existing blocking worker pool, before it shuts
down. Failure to save does not prevent session cleanup.

During use, only resize/move/scale/close events mark the in-memory sample dirty.
Sampling is coalesced after event dispatch. Slint's cached mode flags avoid extra
X11 mode queries during resize, and minimized/zero surfaces cannot overwrite the
normal size. A [small pinned winit X11 patch](../patches/winit-x11-fullscreen.patch)
synchronizes window-manager fullscreen changes into its existing native cache;
queries happen only on `_NET_WM_STATE` changes, without polling.
Native restoration waits for the OS window to exist. This adds no idle worker,
polling timer or per-frame file writes. Forced process termination cannot be
expected to save a new Linux/Windows size.

## Synthetic validation

```sh
cargo test --locked --bin heya window_state::tests
cargo build --release --locked
HEYA_TEST_RENDERER=winit-skia-opengl scripts/ui-offscreen.sh --window-state-check
HEYA_TEST_RENDERER=winit-skia-opengl scripts/ui-offscreen.sh --wayland-window-state-check
scripts/ui-offscreen.sh --close-check
scripts/ui-offscreen.sh --startup-check
```

Both native restart checks use a private D-Bus session and headless KWin. The X11
case runs inside its private Xwayland. They exercise real window-manager resize,
maximize/restore, fullscreen/restore, minimized close and visible reopening,
changed display bounds, corrupt state, and the command-line override. They never
open an account or touch the owner's display/preferences. Only an explicit
`HEYA_TEST_WINDOW_STATE` absolute path enables file persistence in demo mode.

Unit tests cover transitions, bounded reads, validation, atomic replacement,
non-clobbering on invalid data and logical size fitting across DPI changes.
Linux checks do not establish native macOS or Windows runtime compatibility;
those implementations still need tests on their respective operating systems.

## Validation recorded on Linux, 6 November 2026

The optimized X11/Xwayland and Wayland OpenGL restart sequences pass, including
fullscreen entered from maximized, normal-size recovery, minimized close,
corrupt data, changed display bounds and `--size`. The original binary failed the
basic normal-close check because no state file was saved. The unpatched X11
backend additionally saved fullscreen dimensions as normal bounds; the isolated
backend patch fixes that regression.

The existing CPU-renderer startup and close/reopen checks pass, with complete
synthetic process exits in 23–79 ms in the final run. Strict release Clippy with
all targets and the build without default media features pass. The three state
unit tests pass. The broader suite passed 406 tests in the restricted runner;
its seven PDF/private-D-Bus failures all passed when rerun with their existing
sandbox/bus helpers available (10 tests passed in that group). Fourteen opt-in
tests remained ignored. A small host-side check also type-checked the actual
Win32 module against its pinned native bindings; this is not a Windows build or
runtime test.

**Known software-Wayland limit:** reopening fullscreen, exiting fullscreen and
immediately minimizing can block before close is dispatched. A debugger placed
the main thread in `softbuffer::WaylandImpl::buffer_mut`, waiting in its Wayland
queue's `blocking_dispatch` for a buffer release. The normal manual transition
sequence in the previous binary passed; that comparison does not establish that
this exact restored-fullscreen case predates the feature. The full OpenGL
sequences pass. No timing delay, renderer-specific state substitution or extra
worker was added to mask the blocked renderer. Reproduce explicitly with:

```sh
HEYA_TEST_RENDERER=winit-skia-software scripts/ui-offscreen.sh --wayland-window-state-check
```

With no compiler or other synthetic UI check running, one sequential OpenGL/Xvfb
comparison of the previous installed optimized binary and this optimized build
(with persistence explicitly enabled against a fresh temporary file) measured:

| Render CPU p95 | Before | After |
| --- | ---: | ---: |
| Resize, 100 loaded messages | 21.51 ms | 22.49 ms |
| Resize, 10,000 loaded messages | 18.51 ms | 17.35 ms |
| Composer, 10,000 loaded messages | 17.37 ms | 17.66 ms |

There is no consistent regression in this paired smoke measurement. It measures
application/render work on the same offscreen setup, not real compositor latency
or macOS/Windows performance. The resize spike also exists in the baseline
(411 ms render maximum versus 413 ms afterward). The active tracker can be
included in a future benchmark by giving demo mode a fresh absolute
`HEYA_TEST_WINDOW_STATE` path and running
`HEYA_TEST_RENDERER=winit-skia-opengl scripts/ui-offscreen.sh --ui-benchmark`.
