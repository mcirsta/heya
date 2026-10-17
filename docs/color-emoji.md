# Color emoji text rendering

Heya uses the platform emoji font through Slint/Skia for chat text, reaction
badges, previews and the composer. No emoji image overlay, extra control, timer,
or worker is created. Original UTF-8 text remains in the selectable TextInput.
The existing virtual timeline caches presentation rows and exact bubble geometry.

The pinned ZapFast `src/markup.rs::layout` enlarges messages containing one to
three emoji to 2.4 times the body size. Heya applies the same display rule using
Unicode grapheme boundaries and the existing `emojis` database; mixed text and
four or more emoji keep normal text size. Full sequences (ZWJ, skin tone, flags,
keycaps) count as one. Whitespace does not count, and VS15 requests text style.
The visible body and both size probes share the same font size.

## Isolated text-shaper patch

Slint 1.18.1 uses Parley 0.11.1. Parley appends the generic Emoji family after
the requested text-family stack, which in Slint includes generic SansSerif and
SystemUi. On this Linux installation a monochrome fallback covers common faces;
Inter itself covers symbols such as the heart. Those fonts win before Noto Color
Emoji can be tried. The existing picker avoided this with an explicit emoji font.

`vendor/parley` is the unmodified crates.io 0.11.1 source except for
`patches/parley-color-emoji.patch`: prefer the platform's generic Emoji family
for emoji clusters, retaining the original stack if the emoji font lacks a glyph.
An explicit VS15 text selector retains ordinary text-family priority. Digits, #
and * only prefer the emoji font inside a keycap sequence. Latin text,
numbers, punctuation and other scripts retain their existing font stack.
No font paths, Fontconfig changes, bundled font, or renderer changes are needed.

- Base crate SHA256: `22d2ff88bd3f7d68d1d9b09c7e6209f9a8e8c05088295140a2bcf2e9b17038c5`
- Patch SHA256: `e9c1c2c123ae657252b2882664e7264624e667e03c611430f398f9dc5fe5143e`
- Upstream: <https://github.com/linebender/parley>
- Retain the upstream Apache-2.0/MIT notices in `vendor/parley`.
- Candidate upstream fix: emoji-family priority plus explicit text presentation;
  no upstream submission has been made.

On macOS the patch prefers Apple's UI emoji face, which matches the system text
font's ascent and descent, before the generic emoji family. The ordinary Apple
emoji face has different line metrics: with Slint's fixed line height, inserting
an emoji moved the line's baseline up by about 0.5 points at 14.5-point text.
The UI face keeps plain text, mixed text and emoji-only wrapped lines on the same
baseline grid. The generic family remains a fallback if the UI face is absent.

The same patch also contains a macOS-only visual workaround: positioned Apple
Color Emoji glyphs move down by 0.125 em, leaving text, advances and line metrics
unchanged. This applies to both mixed text and emoji-only messages. All added
fields and positioning code are gated by `target_os = "macos"`; Linux's emoji
selection and positioning remain unchanged. The small adjustment compensates
for the elevated baseline observed with Skia's byte-loaded Apple emoji font.
See the [upstream report and standalone reproduction](upstream/macos-emoji-baseline.md)
for evidence and the limits of this workaround.

## Validation

Pure tests cover sizing of complete sequences, text, punctuation and selectors.
The shared Slint test checks original text, narrow/wide resizing, both themes,
measurement and containment. Native pixel checks and optimized timings are kept
in `.prep/validation/color-emoji-20261017/`; these exercise synthetic data on a
private Xvfb display, not the owner's account or compositor.

Linux uses the installed Noto Color Emoji. macOS uses Apple's UI emoji face, with
synthetic checks for mixed text, standalone emoji, sequences, ordinary text and
horizontal spacing, plus wrapped-line baseline checks at 1×/2× display scales
and an offscreen Skia font-loading comparison. Windows
uses its generic platform emoji family and remains unverified.

To reproduce the synthetic display and emoji-heavy timing workload after an
optimized build:

```sh
cargo build --release --locked
HEYA_TEST_RENDERER=winit-skia-opengl scripts/ui-offscreen.sh --framebuffer /tmp/heya-emoji.png --demo-chat 13 --size 1200x800
HEYA_BENCHMARK_CHAT_EMOJI=1 HEYA_TEST_RENDERER=winit-skia-opengl scripts/ui-offscreen.sh --ui-benchmark
```
