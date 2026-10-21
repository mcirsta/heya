# Photo albums in the viewer

The photo viewer follows explicit `MEDIA_ALBUM` associations. It does not guess
from adjacent messages, matching names or timestamps. Album headers remain
invisible metadata; their advertised photo/video counts are not used as the
number of available photos.

The viewer has a sender/date toolbar, fitted image, previous/next controls,
caption, photo counter and a centered thumbnail strip. The neutral canvas,
side chevrons and selected thumbnail treatment follow WhatsApp Web's visual
layout and Heya's existing theme tokens. Original icons are in `ui/icons/viewer-*`.
The public [WhatsApp Web screenshot](https://www.aiseesoft.es/images/tutorial/whatsapp-screenshot/select-a-mode-to-screenshot.jpg)
was used only as a visual reference, without application-code reuse.

Left/Right navigate within the album without wrapping. Space toggles fit/zoom;
Escape closes; Ctrl+C (Command+C on macOS) uses the existing original-image
clipboard action. Changing photos resets zoom and pan. Standalone photos do not
acquire navigation to unrelated chat media. Videos in a mixed album are omitted
from this photo-only viewer; video playback remains its separate existing path.

## Storage and protocol

Archive schema 18 adds `album` and `album_index` columns plus a composite index.
The parent wire ID is scoped to the child's canonical chat and canonical sender,
so different authors/chats cannot accidentally share an album. Supplied indices
sort first, followed by timestamp/local-row order for items without an index.
The header need not be present, which permits incomplete/out-of-order history.

On startup, a one-time batched repair extracts associations from the retained
encrypted envelopes. It updates only these columns, preserving original bytes,
previews, captions, drafts, unread state and message identity. Association data
survives caption edits, alias merges and reopening. Wrapped media is reprocessed
with content decoder revision 11 and unsupported-message repair revision 3.

The pinned SDK's `get_base_message`, `into_base_message` and `is_view_once` did
not recognize `associatedChildMessage`. The isolated `album-child.patch` adds
that wrapper to the existing helpers, including privacy detection. See
[the upstream patch queue](../patches/README.md). No custom crypto, new network
route or protocol-library album type is needed.

The archive resolves membership independently of the loaded conversation page.
Deleted, hidden, expired and locked photos are excluded. The viewer revalidates
on archive changes; an unavailable current item closes the viewer. Remaining
photos have a fresh counter. Missing originals show the existing download/retry
state, and cached originals work offline. Opening a photo is explicit demand,
even when automatic photo downloads are disabled. The filmstrip never marks other album
members viewed or queues their originals; existing chat auto-download preferences
still apply to the photos already visible in the transcript.

## Work and memory bounds

The existing photo presentation/download workers are reused; there is no new
service thread or polling timer. Archive access and image decoding stay off the
Slint loop. Navigation coalesces through the existing latest-value channel.
Only the selected photo has a full decode, capped at a 2048px edge. The strip
holds at most seven 96px previews from cached/embedded thumbnails, centered on
the selected member. Overlapping thumbnails retain their pixels and toolkit
images. No original prefetch or per-photo widget is added to the timeline.
Closing clears viewer pixels, album metadata and strip textures.

Synthetic tests cover wrapped/ordinary children, privacy, missing headers,
explicit ordering, unrelated senders/chats, off-page navigation, mixed media,
caption changes, deletion/expiry/locking, alias merges, schema migration,
cache preservation, reopening, late decodes, coalescing, bounded thumbnail reuse,
light/dark narrow layouts, zoom reset, endpoints and teardown.

The opt-in optimized benchmark uses SQLCipher with 10,000 messages, a 100-photo
album and only one loaded timeline row. After initial renderer warm-up it measures
selection-to-UI readiness, UI publication, resize and closed idle behavior. Render
completion/CPU timings are also reported when the backend exposes frame callbacks:

```sh
HEYA_BENCHMARK_ALBUM=1 scripts/ui-offscreen.sh --ui-benchmark
HEYA_BENCHMARK_ALBUM=1 HEYA_TEST_RENDERER=winit-skia-opengl scripts/ui-offscreen.sh --ui-benchmark
scripts/ui-offscreen.sh --framebuffer /tmp/heya-album.png --demo-chat 9 --size 1200x900 --click 500x100
```

These synthetic offscreen checks do not establish live-account interoperability,
macOS native clipboard behavior, network latency, GPU completion or compositor
presentation timing. Native macOS validation remains required.

## Linux validation — 21 October 2026

Optimized synthetic suites passed: 270 application tests with default media,
252 without it, plus the protocol-storage integration test. Seven explicit
native/opt-in tests remain ignored by the ordinary suites. Strict Clippy passed
for both feature configurations. The separate private-X11 clipboard/UI check
copied a full-resolution PNG to another process and confirmed normal shutdown.
Wide/light and narrow/dark framebuffer captures were inspected offscreen.

The archive-backed viewer benchmark supplies its own model/publisher, with
ordinary demo publishing and automatic chat paging disabled for this fixed
one-row window. Both software and OpenGL runs passed membership, navigation,
resize, cleanup and unchanged-timeline assertions. Navigation did no timeline
layout/measurement work; close retained no viewer image or thumbnail model.

| Measurement | Result |
| --- | --- |
| Before/after 10,000-message photo-scroll render CPU p95 | 12.66 / 12.83 ms |
| Software album UI publication p95 | 1.52 ms |
| OpenGL album UI publication p95 | 2.25 ms |
| Selection to full-photo UI readiness p95, software/OpenGL | 157.23 / 148.91 ms |
| Selection to rendered full photo p95, offscreen OpenGL | 241.64 ms |
| Closed idle timer lateness p95, software/OpenGL | 0.06 / 0.07 ms |

The OpenGL Xvfb run reported unavailable DRI3 acceleration and Mesa fallback.
Rapid resize render CPU p95 reached 77.98 ms in that run, despite UI publication
and timeline layout remaining small; this is not evidence of smooth native GPU
or compositor resizing. The software backend does not expose frame callbacks,
so its render timings are explicitly unavailable. These are local synthetic
measurements, not speed equivalence with another client or live CDN tests.

The subsequent [resize investigation](resize-performance.md) identifies a Slint
opacity-layer allocation issue and records the isolated renderer patch and
paired offscreen measurements.
