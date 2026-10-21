# Received video playback

Common received video messages have a sender-supplied poster, duration and full
caption. Play starts one decoder; the controls pause/resume, seek, mute and
expand it. Expanded and inline views share that decoder. Playing audio or
recording a voice message stops video. Playback is explicit, including videos
marked as GIFs; GIF looping, animated stickers, video notes and sending videos
remain separate work. View-once videos are excluded.

Linux uses the existing GStreamer dependencies. A seekable in-memory `appsrc`
feeds `decodebin`, a bounded video queue, RGBA conversion/scaling and `appsink`.
An audio branch is created only when an audio track is discovered and uses the
configured audio output and volume. Available system plugins determine codec
support. The synthetic native check uses H.264/AAC and a silent audio sink.

macOS uses AVFoundation (`AVURLAsset`, an in-memory resource loader,
`AVPlayerItemVideoOutput`) and Core Image for orientation/scaling. No GStreamer,
FFmpeg executable, Linux helper or plaintext temporary media file is used by
the macOS runtime. This implementation still requires a macOS build and native
playback check; Linux checks cannot establish macOS compatibility.

## Work and lifecycle bounds

The decoder runs in a finite child process, started only for playback. It gets
compressed bytes over an anonymous pipe and sends bounded RGBA frames plus
progress back. Frames are limited to a 960px edge and 25fps. A latest-value
channel and the existing coalescing UI pump prevent frame callbacks from
accumulating. Progress ticks reuse the existing Slint image; they do not rebuild
the transcript or remeasure timeline rows. No player is created for static
posters. A paused decoder retains its playback state but does not repeatedly
upload the same frame.

Stop, switching chats, hiding the conversation, scrolling the active card out of
the materialized timeline, deletion/expiry/locking, and application shutdown
cancel pending downloads and release playback. A hung child has a bounded stop
deadline and is killed/reaped. Normal end-of-stream also waits for process exit.
The command-reader thread and GStreamer/native media threads belong to that
child, so they do not survive it. Expand/collapse intentionally keeps playback.

Playback is capped at 100 MiB and six hours. Download remains available for
larger attachments within its existing limit. Originals are authenticated by
the existing protocol media adapter, stored inside SQLCipher (`message_videos`,
schema 17), and reused for offline playback and Download. A captured message
identity is rechecked after fetching; deletion, replacement and hidden/expired
content cannot be restored by a late cache write. Locked chats cannot start
playback. This cache has the same archive-retention lifetime as other media
caches; it is not a general disk-cache eviction policy.

## Reproducing synthetic checks

Run the application tests with and without default features. For real codec
coverage, pass the Cargo unit-test executable to:

```sh
HEYA_TEST_BINARY=target/release/deps/heya-<hash> scripts/ui-offscreen.sh --video-check
```

This needs the optional test tool `ffmpeg` (`sudo pacman -S ffmpeg`); it generates
temporary four-second landscape, portrait and rotation-tagged H.264 clips with
and without AAC, then exercises decoding, aspect preservation, pause, seek,
mute, end-of-stream and process exit. Fixtures are disposable and
contain no account content. The runtime does not invoke FFmpeg.
The optimized application must also exist at `target/release/heya`; the Python
helper verifies real decoder stop and parent-pipe closure against that binary.

To measure the production decoder-to-UI path, generate a synthetic clip at least
16 seconds long and run:

```sh
HEYA_BENCHMARK_VIDEO_FILE=/tmp/synthetic-video.mp4 \
HEYA_TEST_RENDERER=winit-skia-opengl scripts/ui-offscreen.sh --ui-benchmark
```

The benchmark includes poster, playback, repeated resize, expanded and stopped
phases. Its audio sink is silent. It reports render CPU, event-loop timer
lateness, image/progress application and timeline measurement costs, and fails
if decoding stops during the active phases. These offscreen application timings
do not establish real audio-device, compositor, macOS or account interoperability.
Use `--demo --demo-chat 14` for synthetic video cards.

Visual reference: the pinned ZapFast conversation video tile and bottom
play/time/seek/mute/expand controls recorded in `prep/upstreams.json`. Heya's
decoder protocol, state, storage and media implementation are independent.

## Linux measurement, 21 October 2026

An optimized build with private Xvfb/Skia OpenGL exercised real 1280×720 H.264/AAC
decoding (silent sink), pipe transfer, Slint image publication and rendering:

| Three-second phase | Render CPU p95 | Frame/progress application p95 | Timeline rows remeasured |
| --- | ---: | ---: | ---: |
| Inline playback | 14.55 ms | 0.94 ms | 0 |
| Repeated resize | 14.25 ms | 0.42 ms | 174 |
| Expanded playback | 15.90 ms | 0.47 ms | 3 |
| Stopped | 10.26 ms | 0.02 ms | 0 |

Steady resize and expanded phases each advanced 75 decoder frames in three
seconds, consistent with the 25fps ceiling. The poster phase included a 1.94s
cold first-render sample; warm values above do not describe startup. Stopped
playback returned to four observed settling frames, rather than a continuous
animation.

A separate before/after pair ran the same existing 10,000-message photo/wheel
workload with no compiler alongside either run. Scroll render CPU p95 was
13.21→11.56 ms; layout p95 was 0.17→0.16 ms, with zero rows remeasured during that
phase in either build. Repeated-resize render p95 was 11.61→10.14 ms. These single
pairs establish no slowdown observed in this workload, not a general speedup or
ZapFast/compositor equivalence. Wide, narrow/dark, playing and expanded native
framebuffer captures were inspected on the private display.

Final Linux regression validation passed 263 default application tests, the
encrypted-storage integration, and 245 application tests without default media
features, with seven opt-in tests skipped in each unit run. Strict all-target
Clippy passed with and without default features. Four real codec cases passed
(landscape/AAC, silent, portrait, display-matrix rotation), and explicit Stop
and parent-pipe closure both reaped a playing native decoder. One existing
synthetic session-lock reopen test returned `AlreadyRunning` once in the first
no-media run; it passed in isolation and on the full rerun. The cause was not
established by this video work. No live-account messages were sent by tests.
