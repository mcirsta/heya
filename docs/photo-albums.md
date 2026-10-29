# Photo albums in chat and the viewer

## Compact timeline grids

Consecutive album photos share a two-, three- or four-tile
card. Larger runs shade the last preview and show `+N` for the remaining photos.
Tiles crop to their reserved cells; the viewer keeps the original proportions.
Each tile retains its timestamp and message identity. Clicking the counter opens
that photo in the existing full-album viewer, with navigation to the other members.
Native right-clicks and hover actions target the individual photo under the pointer.

The first photo's caption, quote and reactions stay on the card. A later caption,
quote or reaction starts a separate card, preserving its visible text/state.
Different participants, dates, forwarding state, unrelated messages and deleted
photos also break a run.

Explicit protocol associations take priority. Without an association, consecutive
photos can form a local display album when they have the same canonical sender,
sent/received direction, forwarding state and local calendar day, with at most
60 seconds between adjacent photos. Text, other media, visible deleted items,
explicitly linked photos and later per-photo details break these inferred runs.
Missing or invalid timestamps do not qualify. Both old and new cached messages
use this rule; no envelope reprocessing is needed.

Inferred identities use the first member's local row ID in a distinct key type.
They are transient view metadata, never written to protocol association columns,
envelopes or outgoing messages. Every original message keeps its own identity,
caption, receipts, actions and cached media.

Selection mode exposes the original individual messages for batch operations.
Search and quote navigation to a collapsed member highlight its containing card.

The raw conversation, transcript, message IDs and archive rows remain intact.
Counts use available eligible photos, including cached members outside the current
page; split runs retain their own count. Loading history preserves the position
inside the card. Only four previews per visible card enter the existing bounded
photo demand; the counter does not fetch the remaining originals. Loading previews
does not change card geometry. Text-only timelines skip the grouping allocation.

Synthetic UI checks cover 2/3/4/37/38/1,000 photos, light/dark and narrow layouts,
per-tile context targeting, counter activation, off-page counts, selection and
unchanged geometry during image publication. Archive tests cover count refresh
and association-only invalidation. Reproduce native checks offscreen:

```sh
HEYA_DEMO_ALBUM_GRID=1 scripts/ui-offscreen.sh --framebuffer /tmp/heya-grid.png --demo-chat 9 --size 1200x900 --light
HEYA_BENCHMARK_ALBUM_GRID=1 scripts/ui-offscreen.sh --ui-benchmark
HEYA_DEMO_UNLINKED_ALBUM=1 scripts/ui-offscreen.sh --framebuffer /tmp/heya-inferred.png --demo-chat 9 --size 1200x900 --light
```

These use synthetic photos. They do not establish live-account grouping or
native macOS rendering compatibility.

### Linux offscreen validation — 27 October 2026

325 application tests passed with media and 306 without it; strict Clippy passed
for both configurations. Checks include real pointer and keyboard activation,
per-tile context targeting, caption/quote/reaction retention, long outgoing
status text, selection expansion and image-loading geometry. Light/wide and
dark/narrow native framebuffer captures were inspected. Clicking `+34` opened
photo 4 of 38 in the full viewer. Native synthetic close checks found no idle
media thread and exited in 30–47 ms.

Optimized OpenGL/Xvfb measurements used a synthetic SQLCipher photo cache on the
project filesystem. The control contains 10,000 standalone photo messages; the
album stress contains 10,000 original messages in groups of 38. Results are CPU
timings, not GPU completion or compositor presentation:

| Measurement | Before | After |
| --- | --- | --- |
| Standalone photo scroll render CPU p95, paired repeat | 11.39 ms | 10.94 ms |
| Standalone photo scroll layout p95, paired repeat | 0.17 ms | 0.17 ms |
| Standalone photo resize render CPU p95, paired repeat | 11.09 ms | 9.89 ms |
| Album grid scroll render CPU / layout p95 | — | 13.92 / 0.27 ms |
| Album grid resize render CPU / layout p95 | — | 13.73 / 0.39 ms |

Visible album cards held 8–12 previews across two or three cards, never all
members. The reference-photo repeat remained comparable at p95, but isolated
resize render outliers reached 94.61 ms afterward versus 19.02 ms beforehand
(the first pair reached 96.33 / 12.08 ms). The earlier cold resize phase reached
roughly 400 ms in both builds. These outliers have not been attributed; Xvfb
reported unavailable DRI3 acceleration and Mesa fallback. These results do not
establish smooth native GPU resizing, and the album increment does not claim to
resolve renderer resize spikes.

Baseline binary SHA-256: `656b2edf81784d2b2d4b0ee392ebe60b1ba26471e5b2ec5d97b16a08476ef7e5`.
Album binary SHA-256: `21afe4a293c4da3ca48e5867530f85a40d364c8acde4cf05d58a9a23c9078be1`.
Both use the same pinned dependencies. Reproduce the control with
`HEYA_BENCHMARK_PHOTOS=1 HEYA_BENCHMARK_PHOTO_ARCHIVE=1` and the grid with
`HEYA_BENCHMARK_ALBUM_GRID=1`, passing `HEYA_TEST_RENDERER=winit-skia-opengl` to
`scripts/ui-offscreen.sh --ui-benchmark`. `HEYA_TEST_BINARY` selects a baseline.
The repeat disabled Mesa's shader cache in both runs with
`MESA_SHADER_CACHE_DISABLE=true`; it did not remove the resize outlier.

## Full album viewer

The archive resolves both explicit `MEDIA_ALBUM` associations and inferred photo
batches into the same gallery model. The timeline and viewer share the inference
rule, including members beyond the loaded history page. The viewer has no second
grouping algorithm or separate download path. Album headers remain
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

## Viewer actions

The photo toolbar follows the supplied WhatsApp Web reference: Zoom out/in,
Go to message, Reply, Star/Unstar, Pin/Unpin, React, Forward, Download, More and
Close. Narrow windows keep zoom, Download, More and Close visible; More retains
the other message actions and Copy image. Ctrl+C (Command+C on macOS) still
copies the original image. Zoom steps between fitted, 2× and 4× display size;
Space switches between fitted and 2×.

Go to message opens the selected original in saved chat history. Reply returns
to the composer only after the reply draft is accepted. Forwarding, reactions,
pin duration and More open above the viewer, preserving the selected photo and
its album position. Pin duration offers 24 hours, 7 days and 30 days.

The photo service publishes one eligible message's action metadata with the
existing gallery snapshot. This covers album members outside the loaded chat
page without loading the entire album into the timeline. The archive rechecks
visibility, expiry, lock state and message identity for each operation. Queued
star/pin changes retain the confirmed state until the existing outbox receives
confirmation; errors and pending results stay visible. See
[message controls](message-controls.md) for persistence and synchronization.

The toolbar and icons are Heya's own code. Synthetic checks exercise queue
rejection, delayed results, off-page actions, navigation, modal ordering and
narrow-window access; they do not establish live account interoperability.

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
state, and cached originals work offline. Automatic photo loading keeps only the
available image in chat, album grids and the viewer, without loading labels or
spinners. Manual download and retry controls remain available; the viewer reserves
status text for errors or a missing connection. Opening a photo is explicit demand,
even when automatic photo downloads are disabled. The filmstrip never marks other album
members viewed or queues their originals; existing chat auto-download preferences
still apply to the photos already visible in the transcript.

Inference reads only cached photo metadata for relevant calendar days, using the
timeline index and indexed predecessor lookups to preserve intervening-message
boundaries. It does not decode envelopes or media. Timeline cache hits skip this
work; rebuilding a view queries only days containing a potential loaded pair.
Strict text appends retain existing group metadata rather than scanning those
days again; appending a photo refreshes membership through the same resolver.
A viewer click independently resolves the complete cached run, including when
only one member is loaded in the conversation. Edits, reactions, deletion,
expiry, history import and alias changes use existing archive invalidation.
No persistent inferred-album table, new service or polling timer is added.

The existing optimized benchmarks also accept unlinked-photo fixtures:

```sh
HEYA_BENCHMARK_ALBUM=1 HEYA_BENCHMARK_UNLINKED_ALBUM=1 scripts/ui-offscreen.sh --ui-benchmark
HEYA_BENCHMARK_ARCHIVE=1 HEYA_BENCHMARK_UNLINKED_ALBUM=1 scripts/ui-offscreen.sh --ui-benchmark
```

The first exercises cached originals, gallery navigation and teardown. The
second measures the full SQLCipher-to-UI path with 37-photo runs separated by text
in a 10,000-message conversation.

### Inferred-batch validation — 27 October 2026

331 application tests passed with media and 312 without it; strict Clippy passed
for both configurations. Synthetic checks cover strict photo/text appends,
barriers, reactions, aliases, midnight, paging, reopening and unchanged stored
records/media. A read-only check using the production resolver returned all 37
members of the reported older batch without modifying the archive.

Optimized OpenGL/Xvfb archive measurements found an unnecessary repeated metadata
scan on text appends. Retaining existing groups reduced SQLCipher view/projection
p95 from 58.67 to 10.54 ms at 100 loaded messages and from 73.28 to 15.95 ms at
10,000. Final UI publication p95 was 2.45 / 12.10 ms respectively; unchanged
timeline cache reuse stayed at 0.05 ms. Full rebuilding still has a cost:
99 paging commands took 8.66 seconds in the photo-heavy stress, with projection
p95 of 119.05 ms on the archive worker and UI publication p95 of 18.63 ms.

The separate inferred 100-photo viewer benchmark retained one timeline row,
reported UI publication p95 of 1.57 ms and selection-to-ready p95 of 157.32 ms,
and passed navigation/resource cleanup assertions. Light/wide and dark/narrow
native captures were inspected. Private-display close checks found no idle
media thread and exited in 24–47 ms. These are synthetic offscreen measurements
with Mesa fallback, not native GPU/compositor or macOS validation.

## Work and memory bounds

The existing photo presentation/download workers are reused; there is no new
service thread or polling timer. Archive access and image decoding stay off the
Slint loop. Navigation coalesces through the existing latest-value channel.
Only the selected photo has a full decode, capped at a 2048px edge. The strip
holds at most seven 96px previews from cached/embedded thumbnails, centered on
the selected member. Overlapping thumbnails retain their pixels and toolkit
images. Gallery navigation adds no original prefetch or extra timeline widgets.
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

## Viewer-action validation — 29 October 2026

345 Linux tests passed with media and 326 without it, with strict Clippy in both
configurations. The single-toolkit-thread suite exercises the real toolbar and
shared callbacks with synthetic services: rejection, delayed reply results,
Star/Pin confirmation, duration selection, off-page photo actions, More on narrow
windows, navigation and stale-result matching. Zoom is checked at fitted, 2× and
4× sizes. Native light/wide and dark/narrow captures cover the viewer, reaction
picker, forwarding chooser, pin-duration dialog, More and return to the composer.

The smooth-wheel regression now uses a separate joined test thread with a mock
clock. Deterministic 16 ms frames preserve checks for intermediate positions,
monotonic travel, reversal and rapid-wheel accumulation without depending on
CI scheduler delays. Application scrolling is unchanged.

Optimized OpenGL/Xvfb comparisons used the same synthetic SQLCipher fixture
(10,000 messages, 100 album photos, one loaded conversation row) on the project
filesystem, with Mesa's shader cache disabled. The first toolbar implementation
recolored disabled SVG icons on each selection; repeated navigation render CPU
p95 rose to 27.78–32.89 ms. Keeping ink stable and changing opacity removed that
cache invalidation. Reusing the default empty actions model also avoids allocating
a different model for every otherwise identical publication. Row comparison now
includes wire/time/operation metadata, preserving capability and mark updates.

| Measurement | Baseline | Final run |
| --- | --- | --- |
| Navigation render CPU p95 | 18.41 ms | 20.20 ms |
| Settled resize render CPU p95 | 10.11 ms | 10.88 ms |
| Selection to UI ready p95 (19 samples) | 149.29 ms | 159.75 ms |
| Selection to render callback p95 (19 samples) | 225.16 ms | 235.72 ms |

The intermediate rendering-fix repeat measured navigation p95 at 19.76 ms and
selection-to-render p95 at 211.73 ms. Extra controls retain a small measured
rendering cost; these runs do not establish identical performance or native
GPU/compositor behavior. The final resize maximum remained 153.89 ms, comparable
to the baseline's 149.97 ms; the existing renderer resize spikes remain open.
Every run retained one timeline row and released the viewer on close. Separate
synthetic close checks found no idle media thread and exited in 24–47 ms.

Baseline source: `0bea7ec`; binary SHA-256:
`4d7344cfa8861d6ebb2dc584697d13c0cc22cd633613546f2b66d750badc7d28`.
Final tested binary SHA-256:
`d217817cc6383836ad9361d6a940ae909db19d4670288e5c0f8e14f00b73d6c1`.
Reproduce with `HEYA_BENCHMARK_ALBUM=1 HEYA_TEST_RENDERER=winit-skia-opengl
MESA_SHADER_CACHE_DISABLE=true scripts/ui-offscreen.sh --ui-benchmark`; set
`TMPDIR` to a disposable directory on the data filesystem and use
`HEYA_TEST_BINARY` to select a baseline.
