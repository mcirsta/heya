# Received static stickers

Static WebP stickers render directly on the chat background, with a compact
timestamp and the usual message actions. Download saves the original WebP;
Copy image preserves transparency. Stickers are separate messages and do not
enter photo albums or shared-photo galleries. Sending stickers is not added.

The existing photo worker/cache handles demand, automatic downloads, explicit
retry, cancellation and shutdown. No additional worker or idle thread is added.
Authenticated SDK downloads are capped at 1 MiB for stickers. Bounded decoding
rejects malformed images and animated WebP; alpha survives the PNG display
preview. SDK-flagged animated/Lottie stickers show `[Animated sticker]` until
animation playback is implemented. View-once media remains unopened.

Startup repair revision 5 upgrades eligible saved `[Unsupported message]` rows
in bounded transactions, updating text, dimensions and thumbnail together.
Message identity, timestamps, read state and raw envelope stay unchanged.
Deleted, edited, expired and hidden rows are excluded; a completed revision
avoids rescanning the archive on subsequent startups.

Synthetic tests exercise envelope mapping, old-message repair, alpha decoding,
cache/download/clipboard guards, delayed/failing transfers and cancellation.
The existing offscreen toolkit test checks light/dark, narrow/wide, direct/group
layouts and stable geometry while pixels arrive. These tests do not establish
real-account CDN availability or macOS interoperability.

For a synthetic screenshot, use
`HEYA_DEMO_STICKERS=1 scripts/ui-offscreen.sh --framebuffer /tmp/heya-stickers.png --demo-chat 9`.
For the existing optimized native benchmark with sticker rows, use
`HEYA_BENCHMARK_STICKERS=1 scripts/ui-offscreen.sh --ui-benchmark`.

Validation on Linux, 2026-10-30: 360 tests with media and 341 without media,
plus the protocol-storage integration test in both configurations; strict
all-target Clippy passed in both. Offscreen screenshots checked transparency
and hover actions. Optimized, sequential private-X11 runs used animated wheel
scrolling and the encrypted image cache:

| 10,000-message workload | Scroll layout p95 | Resize layout p95 |
| --- | ---: | ---: |
| Photos, previous build | 0.10 ms | 0.83 ms |
| Photos, sticker-enabled build | 0.12 ms | 0.94 ms |
| Static stickers | 0.10 ms | 0.52 ms |

Changing-snapshot application p95 was 0.25 ms for both photo runs and 0.27 ms
for stickers. Sticker residency stayed at 4–6 visible image rows. These are
application/layout timings; Skia software frame counters and desktop compositor
timings are unavailable in this offscreen check.
