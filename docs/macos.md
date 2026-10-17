# macOS development

macOS uses Apple platform services wherever an integration is available. The
application remains Rust with Slint's custom interface. Slint/Winit provides
AppKit windows, native text-input/clipboard integration and accessibility; Skia
uses Metal. This does not turn the custom Slint controls into AppKit widgets.

## Build

Use macOS 11 or newer. Install Apple's Command Line Tools (`xcode-select --install`) and Rust 1.99 or
newer from [rustup](https://rustup.rs). Full Xcode, Homebrew, GStreamer, GTK,
Poppler and X11 are not required for the macOS runtime.

```sh
python3 scripts/check_environment.py
cargo build --release --locked
target/release/heya
```

Normal startup resumes a saved session or presents the connection screen.
`target/release/heya --demo` uses synthetic data without opening an account.
Use `scripts/build-macos.sh` to build a local `target/macos/Heya.app` bundle.
This is an ad-hoc-signed development app, not a notarized distribution.

## Native integrations

| Service | macOS implementation |
| --- | --- |
| Windowing, input, clipboard, accessibility | AppKit through Winit/Slint |
| Rendering | Skia over Metal |
| Fonts | System interface font by default, Apple Color Emoji |
| Keyboard shortcuts | Command for app actions, copy and send; Slint normalizes Command to its logical Control modifier |
| Archive encryption key | Apple Keychain through keyring's `apple-native` backend |
| Open/save/folder dialogs | Asynchronous NSOpenPanel/NSSavePanel sheets on the main thread |
| PDF thumbnail generation | CoreGraphics/Quartz and ImageIO in a disposable helper process |
| URL opening | Native system browser dispatch through webbrowser |
| App data and Downloads paths | Standard macOS locations through directories |
| Audio/video playback and recording | Disabled; future work must use AVFoundation/Core Audio |

GStreamer dependencies and their implementations are gated to Linux, including
when the default feature is enabled on macOS. No Mac GStreamer installation is
needed. Linux retains GStreamer audio, desktop portals, Secret Service,
sandboxed Poppler PDF previews and Skia's Vulkan/OpenGL renderers. Apple-specific
dependencies, service implementations, fonts and layout adjustments are scoped
to macOS; shared application features continue to support both platforms.
SQLCipher and the WhatsApp protocol implementation remain shared application
dependencies. Apple's ordinary SQLite cannot replace SQLCipher without losing
the encrypted archive; the vendored OpenSSL dependency supplies its encryption.

The small Objective-C bridge is `src/platform/macos.m`; Rust owns cancellation
and result delivery in `src/macos.rs`. No Objective-C object or raw window pointer
crosses a worker thread. Dropping or cancelling a file-picker request schedules
closure of the native panel. PDF input and PNG output travel through pipes,
never plaintext document-cache files. The helper has an output bound and an
eight-second deadline; it runs before any account initialization. It is a
separate process, not an App Sandbox or XPC service.

## Validation

Validated on an Intel Mac running macOS 15.8.1 with Rust 1.99.1 and Apple's
Command Line Tools 16.4 (Clang 17): 174 unit/UI tests and one encrypted protocol
storage integration test passed. Three opt-in/helper tests remained ignored,
including the actual desktop Keychain check. Formatting, all-target Clippy with
warnings denied, and the build check without default features passed. The
vendored Parley crate still emits its existing ICU deprecation warning.

The Objective-C bridge also passes Clang with `-Wall -Wextra -Werror`. The
compiled release PDF helper renders all three fixtures and rejects corrupt PDF
input. The UI suite covers Command shortcuts using Slint's normalized key
events, and a macOS-only minimum message height keeps the menu target contained
with compact Apple fonts. Linux retains its existing message sizing.
Apple emoji also have a small macOS-only baseline workaround, with real-font
regression coverage and a [draft upstream report](upstream/macos-emoji-baseline.md).

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo check --locked --no-default-features
```

The Linux GitHub Actions workflow compiles, lints and runs the synthetic suite
both with default GStreamer audio and with `--no-default-features`. It runs on
Linux with the real Linux dependencies, including the PDF sandbox and media
plugins. Mac checks alone cannot establish Linux build or runtime compatibility.

The existing synthetic PDF fixtures cover portrait, landscape, rotated crop
boxes, invalid input, thumbnail detail, and encrypted-cache presentation. Native
file dialogs, Keychain access, IME, paste, display scaling and suspend/resume
still require deliberate desktop checks. Do not infer real-account compatibility
from a successful compile or synthetic tests. Native playback/recording,
notifications, signing/notarization and installer distribution remain future work.
