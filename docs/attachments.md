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
PDF document thumbnails, image editing, video-message sending, clipboard/drop sending and persistent attachment-review drafts remain
separate work.

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

Synthetic tests cover immutable photo bytes, image/document construction and
quotes, 32 MiB streaming encryption/decryption with MAC/both hash checks, independent
reader offsets, cancellation and encrypted-file lifetime, invalid/special/oversized
files, batch captions, partial failure/retry, unchanged composer drafts and recipient,
offline/archival failures, active-upload close and idempotent outbox insertion.
The private D-Bus portal fixture covers OpenFile multiple selection, image filters,
Unicode local URIs, early responses and the existing cancellation path.

The corrected full suite passed 157 application/service/UI tests plus the protocol-storage
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
