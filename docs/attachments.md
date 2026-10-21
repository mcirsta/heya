# Sending files and photos

The composer + menu offers Photos and Documents. The Linux OpenFile portal
supports selecting up to ten files; Heya then shows the recipient, previews,
names, sizes and per-file captions. Send is explicit. Selecting/removing files
and editing captions does not change the separate text-composer draft. A captured
quoted reply is applied to the attachments, and another reply draft cannot
silently replace it during upload.

Photos accept JPEG/PNG originals up to 20 MiB and 32 megapixels. They are sent
without recompression. Their small JPEG thumbnail and orientation-aware review
image are generated locally. Documents preserve arbitrary original file bytes;
an image chosen as Document uses a document message. The selection is limited
to 2 GiB in total, with a ten-minute timeout for each upload. Captions are limited
to 1,024 Unicode characters; over-limit text is retained and rejected rather
than silently shortened. These are Heya policies, not reverse-engineered WhatsApp
limits. Recorded [voice messages](voice-messages.md) now reuse this transfer/outbox path.
PDF document thumbnails, image editing, video-message sending and persistent
attachment-review drafts remain separate work.

The composer also accepts Ctrl+V (Command+V on macOS) or Shift+Insert for
clipboard photos/files, and native file drops. File lists take precedence over
clipboard thumbnails; plain paths copied as text stay text. JPEG/PNG files are
staged as photos and other dropped/pasted files as documents. A single drop is
collected into one bounded review, with repeated paths removed. Both routes
reuse the same encrypted staging, review and explicit Send operation. The
recipient is captured before asynchronous work, and the separate text draft is
preserved. An existing attachment review or recording must be finished first.

Text paste stays in the composer at its selection, supports native undo, and
never invokes Enter-to-send, including multiline text. If the editor, selection
or chat changed while the clipboard owner was being read, Heya asks to paste
again rather than inserting at a different location. Search and dialog editors
retain their normal text paste handling. Primary-selection paste is unchanged.

Photo menus and the photo viewer offer **Copy image** once the photo is loaded.
This copies decoded original pixels with orientation and alpha preserved, not a
filename or the downsampled chat thumbnail. Cached originals work offline;
deleted, expired, wrong-chat and locked media are rejected again at access time.
Clipboard reads, encoding and original-image decoding run on finite preparation
workers, not on Slint's event loop. There is no clipboard polling loop. The
platform clipboard provider must continue serving its current offer while Heya
owns the clipboard; this is distinct from a preparation/upload worker.

The existing arboard dependency supplies X11/Wayland clipboard integration and
native macOS NSPasteboard support. Slint's pinned development backend uses
winit 0.31.0-beta.3, including native Wayland file drops. One window-level
`DropArea` accepts file lists over the conversation and delivers each completed
transfer once. Heya no longer collects per-file events with a timer or maintains
its own drag-hover state. Decoding and preparing selected files still runs in
the existing finite attachment worker.
No XWayland fallback is needed for drops. Native macOS
clipboard/drop acceptance and broader desktop-source compatibility still need
platform testing. Native Wayland clipboard transfers have been checked on a
private KWin compositor with `DISPLAY` unset; compositors without the clipboard
data-control protocol depend on arboard's XWayland clipboard fallback.

Preparation validates regular readable files, size and modification metadata.
Unix opens reject symlinks and do not block on a substituted FIFO. The pinned
SDK streaming encryptor creates private temporary ciphertext and authenticated
hashes before review. Original file changes/removal after preparation cannot
change that ciphertext. Decoding uses the existing photo bounds; at most ten
512px review images remain resident. Only ciphertext is written to staging disk,
in unnamed temporary files. Positioned readers share the file safely without
sharing a seek cursor. Closing the last handle releases storage; process exit
also releases it after an abrupt close, without leaving a staging pathname.
This is not a persistent upload-recovery registry.

After Send, files upload serially through the SDK's streaming API. Readers use independent offsets
into the immutable encrypted file for SDK retry/resume; they never materialize
a large plaintext file in RAM. The pinned Bot's Ureq HTTP client supports streamed upload. A permit retained by blocking readers prevents
rapid cancellation/reconnect from multiplying outstanding upload bodies.
Cancellation is checked during preparation and reads, and a source disconnect
cancels the async upload. Blocking network calls may take time to unwind.

Each successful upload becomes an ImageMessage or DocumentMessage, including
caption, filename/MIME, size, crypto metadata, photo dimensions/thumbnail and
optional quote. The archive inserts it into the existing durable outbox before
removing it from the selection. A staging item keeps the same message ID across
upload/archive retries, so a lost UI completion cannot duplicate a queued row or
reset its sending/delivery state. Chat permissions and quote eligibility are
checked again when queueing. No archive schema or SDK patch was needed.

Once queued, the ordinary outbox/ACK/receipt paths apply. An uncertain message
attempt is not automatically resent. Upload failures keep only the remaining
files/captions for Retry. Cancel during upload retains those unsent items;
Close discards the unsubmitted selection. Queueing is a short commit boundary
where Cancel is disabled. Closing the application cancels outstanding work and
allows an already-started commit to finish; queued messages remain durable.
Selections/captions not yet queued are session-only and are not recovered after
application close or crash. An uploaded object whose archive commit failed is
not itself a sent message.

All selection, filesystem, encryption, image and network work stays off Slint's
event loop. Preparation runs on a finite `heya-prepare` thread. Upload uses a
transfer-local current-thread Tokio executor (`heya-upload`), with at most one
blocking HTTP worker (`heya-upload-io`) and no idle keepalive. This isolates the
SDK's synchronous upload and resume-check work from Heya's shared blocking pool.
The executor joins its blocking work before reporting success, and its owning
thread is joined before the upload result is returned. There is no upload worker
or executor waiting for the next file. Heya's ordinary shared protocol/runtime
workers continue to serve the connected app.

Dropping/cancelling the upload signals the transfer worker. Normal completion,
including an ordinary error result, disarms the abandonment guard and preserves
the caller's cancellation state. Its SDK future is
dropped promptly, releasing Client/session ownership, and body readers check
cancellation between reads. A synchronous OS/network call already in progress
can take time to unwind; cancellation is not a promise of instant socket abort.
Closing the application uses the existing bounded shutdown deadline and exits
the process if such work is stuck. These workers run inside Heya, never in a
separate process, and unnamed file storage is released on process exit too.
Published upload clients are cleared alongside audio/download sources before
the SDK store-release fence.

A single conditional review surface owns a maximum of ten preview controls.
After Send it closes and a compact, nonmodal progress card leaves typing and
history scrolling available. On failure, its review action reopens only the
remaining items/captions. Progress is coalesced to at most ten updates per second.
Preview/model identity is retained during progress; there are no per-message
upload widgets or polling timers.

## Validation

The 21 October 2026 clipboard checks cover Unicode/multiline text replacement
and native undo, stale editor/recipient rejection, image/file precedence,
deduplication and batch bounds, encrypted staging, explicit Send, and access to
original photos after lock/deletion/expiry. The full synthetic suites passed
254 tests with the default media feature and 236 without it, with opt-in
desktop checks run separately. The protocol-storage integration test passed too.

Native clipboard checks separately passed on private Xvfb and headless KWin
displays. Another process supplies CRLF file URIs and reads the copied PNG;
the test checks exact original dimensions/alpha after the preparation worker
has exited. The X11 UI harness also exercises a real PNG paste, an Xdnd drop of
two Unicode-named files, and Copy image through the viewer button/shortcut.
After copying, it verifies no Heya preparation/upload worker is left idle and
that a normal native window-close request exits the process within three seconds.
These tests use synthetic data, not the owner's clipboard or account.

The Slint/winit 0.31 migration also has a real native Wayland drag test on a
private headless KWin compositor, with `DISPLAY` unset. A separate Qt sender
exercises the compositor protocol and the actual Slint `DropArea`: two paths
including Unicode arrive in a single command, cancellation clears hover,
plain text is rejected, and no message is sent or draft changed. Qt is only a
test sender, not an application dependency. The test bus disables desktop
service autostart and its compositor exits after the check.

Run `HEYA_TEST_BINARY=<unit-test-executable> scripts/ui-offscreen.sh --wayland-drop-check`.
It needs KWin, qdbus6, Qt6 Widgets/Wayland, Wayland development headers/scanner,
plasma-wayland-protocols and a C/C++ compiler. All input and windows stay on the
private compositor. The existing X11 sender now waits for asynchronous drop
acceptance while the receiver reads its URI offer.

Reproduce using `scripts/ui-offscreen.sh --clipboard-ui-check /tmp/heya-clipboard`.
For clipboard round trips, set `HEYA_TEST_BINARY` to the unit-test executable
printed by `cargo test --release --locked --no-run`, then invoke
`scripts/ui-offscreen.sh --clipboard-check` and
`scripts/ui-offscreen.sh --wayland-clipboard-check`. The latter needs KWin,
`dbus-run-session`, `wl-copy` and `wl-paste`; the X11 harness needs Xvfb,
`xauth`, `xclip`, `xdotool`, ImageMagick and a C compiler/Xlib headers.

An optimized offscreen comparison against `ae5c528` used
`HEYA_BENCHMARK_PHOTOS=1 scripts/ui-offscreen.sh --ui-benchmark`, sequentially
with no build running. The Skia CPU/Xvfb results were:

| p95 application timing | Before | After |
| --- | ---: | ---: |
| Unchanged snapshot application | 0.04 ms | 0.04 ms |
| Changing snapshot application | 0.24 ms | 0.25 ms |
| Scroll layout, 100 messages | 0.30 ms | 0.32 ms |
| Scroll layout, 10,000 messages | 0.10 ms | 0.10 ms |
| Resize layout, 10,000 messages | 0.87 ms | 0.75 ms |
| Composer layout, 10,000 messages | 0.02 ms | 0.09 ms |

Photo residency stayed bounded at three/four visible inline images. This single
before/after comparison found no material scrolling regression; it is not a
GPU/compositor frame-rate measurement, a Wayland performance measurement, or an
end-to-end account/clipboard transfer-latency benchmark. Clipboard decoding,
encoding and file preparation remain outside the Slint event loop.

The toolkit migration was compared separately against the installed Slint 1.18.1
build, with the same synthetic photo benchmark and no build running:

| p95 application timing | Slint 1.18.1 | Pinned Slint 1.19.0 / winit 0.31 |
| --- | ---: | ---: |
| Unchanged snapshot application | 0.04 ms | 0.04 ms |
| Changing snapshot application | 0.17 ms | 0.22 ms |
| Scroll layout, 100 messages | 0.30 ms | 0.29 ms |
| Scroll layout, 10,000 messages | 0.09 ms | 0.10 ms |
| Resize layout, 10,000 messages | 0.75 ms | 0.82 ms |
| Composer layout, 10,000 messages | 0.03 ms | 0.02 ms |

This run found no material scrolling/typing regression. The five initial-layout
samples reached 16.39 ms before and 22.05 ms after; that small cold-start sample
is not a steady-state performance result. Photo residency remained at three or
four inline images. The migration passed 254 default-feature tests, 236 tests
without GStreamer, encrypted-storage integration, strict all-target Clippy in
both configurations, native clipboard/drop checks, and private OpenGL/wheel and
startup/shutdown checks. macOS runtime validation still belongs to macOS CI and
an actual Mac; Linux tests do not establish it.

A second sequential comparison used `HEYA_TEST_RENDERER=winit-skia-opengl`
with the same photo fixture on private Xvfb/Mesa. It measured actual render CPU
time (unlike the zero frame counters of the Skia CPU backend):

| p95 render CPU | Slint 1.18.1 | Pinned Slint 1.19.0 / winit 0.31 |
| --- | ---: | ---: |
| Scroll, 100 messages | 11.57 ms | 10.00 ms |
| Scroll, 10,000 messages | 10.04 ms | 9.76 ms |
| Resize, 10,000 messages | 9.79 ms | 8.88 ms |
| Composer, 10,000 messages | 11.53 ms | 11.35 ms |

These single-run results show no regression in this offscreen workload; they
are not hardware-GPU, compositor-animation or native Wayland frame timings.
Real X11 wheel events still produced smooth intermediate offsets through the
existing Slint wheel animation. The final OpenGL screenshot was inspected, and
the native software startup check confirmed first-redraw return and shutdown.

Synthetic tests cover immutable photo bytes, image/document construction and
quotes, 32 MiB streaming encryption/decryption with MAC/both hash checks, independent
reader offsets, cancellation and encrypted-file lifetime, invalid/special/oversized
files, batch captions, partial failure/retry, unchanged composer drafts and recipient,
offline/archival failures, active-upload close and idempotent outbox insertion.
The private D-Bus portal fixture covers OpenFile multiple selection, image filters,
Unicode local URIs, early responses and the existing cancellation path.

The original upload slice's corrected suite passed 157 application/service/UI tests plus the protocol-storage
integration test, including queue idempotency and transfer-worker lifecycle. The
real-keyring test remains opt-in. The real Slint workflow checks a 1,000-update
progress burst without changing the timeline/model/preview identity, explicit
callbacks, and overlay/control containment at four window sizes. Protocol shutdown
tests retain a published upload client to verify the ownership fence releases it.
A worker test stalls actual executor blocking work, checks that completion waits
for it, cancels the owning SDK future, and checks that no named upload threads
survive completion. The service test closes its command channel during upload;
these are synthetic lifecycle checks, not a live-network shutdown assertion.
Final focused checks, native offscreen captures, performance measurements and
live-owner evidence are recorded in [the UI audit](ui-source-audit.md).

On 17 October 2026, after the false-cancellation fix, the owner confirmed that
the manually sent attachment appears in Heya and arrives in another WhatsApp
client. Its type and size were not specified; this confirms the tested transfer,
not every photo/document variant or live cancellation/recovery behavior.

Automated tests use only synthetic files/accounts and never send an account
message. SDK primitives and mocked services do not establish live account,
server, recipient or compositor compatibility. The supplied Windows desktop
findings and their limits are in [the desktop audit](desktop-media-audit.md).
