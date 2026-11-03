# Video posters before playback

The existing image-preview worker first reuses a completed cached poster, then
tries a supplied `thumbnailDirectPath` with its own two hashes and the parent
media key under `WhatsApp Video Thumbnail Keys`. A valid supplied image wins.
When it is missing or unusable, an ordinary video generates a poster from an
already cached original, or downloads an initial video portion for extraction.
GIF-style videos do not take this generation path. Wrapped/direct view-once
videos, locked/deleted/hidden/expired messages and invalid references are excluded.

This implements the behavior independently from the current WhatsApp Web modules
`WAWebMediaDownloadMmsThumbnail`, `WAWebDownloadVideoThumbnail`,
`WAWebGetMediaDownloadByterange` and `WAWebVideoPreview.react`, inspected on
3 November 2026. The [public bundle](https://static.whatsapp.net/rsrc.php/v4i9SA4/yv/l/en_US-j/q27BKpArz0D.js)
defines the 480px edge, 128 KiB minimum and estimated one-second initial range.
Web's 1.5-second retry is restricted to contexts without a chat; Heya's nearby
conversation cards use the normal one-second request. No WhatsApp application
implementation is copied into Heya.

## Bounds and ownership

Network prefixes are capped at 4 MiB and aligned to AES blocks. The SDK owns
media-route validation/refresh, host failover, the ranged HTTP request and CBC
prefix decryption. A range-ignoring server cannot force a full original download.
Whole encrypted responses still receive MAC/hash checks. Partial prefixes lack
the complete-file MAC, just as on Web's partial-media route: they are explicitly
preview-only, fed to a finite untrusted-input decoder, and never written to the
verified original cache or exposed through playback/Download. Normal downloads
retain their existing full integrity checks.

The current serial document/image-preview worker and encrypted-or-plaintext
profile cache also handle video posters; no new parked worker is created. A
40ms demand debounce bounds wheel bursts. At most 32 nearby message IDs are
considered. A network attempt has a 12-second deadline; the finite decoder has
an 8-second outer deadline including process startup and pipe transfer. It
exits after one usable frame and is reaped before publication. Dropping work
closes/kills the helper through the existing pipe lifecycle. Linux uses silent
GStreamer decoding; macOS uses AVAssetImageGenerator without creating AVPlayer
or an audio output. No plaintext media file is created.

The result is a JPEG/PNG-derived image up to 480px, preserving orientation and
aspect ratio without upscaling small embedded pictures. PNG storage is bounded
to 1 MiB. All-black/undecodable frames retain the old embedded fallback. Failed
attempts use the existing one-hour retry cooldown, reconsidered only on activity;
there is no retry polling timer. Navigation, disconnect, locking, mutation and
shutdown cancel obsolete work. Captured raw-envelope identity guards every
cache write. Publication replaces the image revision without rebuilding the
timeline, changing row geometry or repeatedly decoding receipt updates.
A newly verified original from normal playback clears a failed poster's cooldown
once; an unsuccessful attempt with that original restores the normal cooldown.

Files whose MP4 metadata is outside the bounded prefix may keep their embedded
poster until a full cached original is available. Decoder-disabled builds can
still use supplied/cached images. Linux synthetic checks do not establish macOS
or real-account interoperability.

## Reproduction

SDK patches and exact base are in [the patch queue](../patches/README.md).
Application tests exercise selection/privacy, tiny-to-sharp cache replacement,
unchanged geometry, stale writes, reuse and finite-child completion. SDK tests
cover range sizing, bounded reads when Range is ignored, full-response integrity
failure, and separation of preview prefixes from authenticated originals.
`scripts/ui-offscreen.sh --video-check` also generates landscape, portrait,
silent and rotation-tagged synthetic H.264 clips and checks one-frame extraction
before normal playback. It never uses an account or real audio output.

## Validation — 3 November 2026

The Linux application suites passed 369 tests with media and 350 without it,
plus the protocol-storage integration test in each configuration. Strict
all-target Clippy passed in both configurations. The isolated SDK tests passed
two range/transport checks and 41 core download/crypto checks. Native GStreamer
checks passed all four synthetic clips above, including a bounded fast-start
prefix, and confirmed decoder release on Stop and closed parent pipes.

Sequential optimized private-X OpenGL runs compared the installed build with
this build on the same 10,000-message photo/wheel workload. Scroll render CPU
p95 was 11.62 versus 11.69 ms; repeated-resize p95 was 10.68 versus 11.01 ms.
Unchanged updates drew no frames and applied at 0.06 versus 0.07 ms p95. These
single-run differences do not establish a meaningful regression. Software-mode
scroll layout p95 was 0.10 ms in both builds; resize layout was 0.83 ms in both.
Software mode does not expose render counters, so its zero frame count is not
a rendering measurement.

The separate real-decoder video benchmark passed playback, resize, expanded
viewer and stopped phases. Render CPU p95 was 14.98, 12.83 and 15.79 ms in the
three active phases; stopped decoding advanced zero frames. Cold OpenGL startup
still took about two seconds in both builds (1.95 versus 1.92 seconds in the
photo comparison). This existing startup outlier is not fixed by poster work.

These checks separately exercise protocol selection, extraction, guarded cache
publication and stable UI geometry. The scrolling benchmark uses prepared photo
data; it does not measure a real-account automatic-poster download end to end,
the owner's compositor, or native macOS decoding.
