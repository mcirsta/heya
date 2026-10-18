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
This is an ad-hoc-signed development app, not a notarized distribution. Use this
bundle for microphone recording: its Info.plist contains the macOS microphone
permission description. The unbundled Cargo executable cannot request that permission.

## Saved encryption key

Heya stores one random master key in one native Keychain item (`org.heya.Heya`,
account `master-v1`). It reads that item once to open both databases; separate
session and archive keys are derived with HKDF-SHA256. Linux uses the same
single-item design through Secret Service. Choosing **Always Allow** authorizes
that build to reopen its saved data. An ad-hoc-signed rebuild can require
authorization again, but session and archive access no longer cause two prompts.

This pre-release change starts a fresh `default-v2` data profile. Scan the QR
code again; existing `default` data and the former `session-v1`/`archive-v1`
keyring entries are left untouched and are not migrated or accessed.

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
| Audio playback | AVAudioPlayer, including Ogg/Opus from memory |
| Voice recording | Core Audio input queues and AudioToolbox Opus encoding |
| Video playback | Pending |

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

## Native audio

Playback uses AVAudioPlayer with in-memory compressed data, pause/resume, seeking
and speed control. Microphone capture uses AudioQueue; AudioToolbox downmixes,
resamples and encodes 48 kHz mono Opus at 32 kbit/s. The Rust `ogg` dependency
packages the native encoded packets; it is not an audio codec. Compressed and
intermediate CAF data stay in bounded memory, with no plaintext audio files.
Recording and review playback use the existing finite helper process. Discovery
does not open the microphone, and permission is requested only when recording.
Settings → Audio uses Core Audio device identities and per-operation volume.
Explicit unavailable devices fail instead of switching to another device.

To test, launch the bundle, open a chat with an empty composer, press the
microphone and allow access. Record a few seconds, pause/resume, then Stop and
preview. Discard is sufficient to test without sending. Also try a received voice
message, seeking and playback speeds. If permission was denied, enable Heya in
System Settings → Privacy & Security → Microphone.

## Validation

Validated on an Intel Mac running macOS 15.8.1 with Rust 1.99.1 and Apple's
Command Line Tools 16.4 (Clang 17): the original 176 unit/UI tests and one encrypted protocol
storage integration test passed. Three opt-in/helper tests remained ignored,
including the actual desktop Keychain check. Formatting, all-target Clippy with
warnings denied, and the build check without default features passed. The
vendored Parley crate still emits its existing ICU deprecation warning.

Native audio validation additionally covers in-memory Opus encode/decode, mono/stereo
44.1/48 kHz conversion, exact decoder trimming, playback controls, recording
pause/resume, review, encrypted caching and helper teardown with synthetic audio.
These checks never open a real microphone or audible output.

The Objective-C bridge also passes Clang with `-Wall -Wextra -Werror`. The
compiled release PDF helper renders all three fixtures and rejects corrupt PDF
input. The UI suite covers Command shortcuts using Slint's normalized key
events, and a macOS-only minimum message height keeps the menu target contained
with compact Apple fonts. Linux retains its existing message sizing.
Apple emoji also have a small macOS-only baseline workaround, with real-font
regression coverage and a [draft upstream report](upstream/macos-emoji-baseline.md).
The macOS fallback prefers Apple's UI emoji face so wrapped emoji and text lines
keep the same baseline spacing; regression checks cover 1× and 2× display scales.

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
from a successful compile or synthetic tests. Real microphone permission, hardware capture/output, and recipient playback still
need deliberate checks. Notifications, signing/notarization and installer
distribution remain future work.
