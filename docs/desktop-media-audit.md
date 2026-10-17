# Windows desktop media audit — 16 October 2026

Reference: owner-supplied `WhatsAppDesktop_2.2640.100.0_x64__cv1g1gvanyjgm`.
IDA CIL inspection of `WhatsApp.Root.dll`, SHA-256
`150a692e7498c046ebf6cef39f9994bdeec2772c53172399d0f827e420579d17`.
Addresses below are IDA method addresses in that assembly, not protocol IDs.
No owner files, messages, credentials or CDN references are included here.

## What the supplied build establishes

- `WaWebEnvironmentConfig..cctor` at `0x39530` initializes Production with
  `https://web.whatsapp.com/`. `TryCreate` at `0x392f0` also exposes development
  and custom environments. `MainWindow.StartNavigation` at `0x34f00` loads that
  URL in WebView2; the bootstrap state machine at `0x695b0` attaches the web
  content updater and native bridges.
- Root assembly thumbnail methods concern link metadata and notifications.
  The supplied `WhatsApp.Design.dll` thumbnail methods concern avatars and
  lightbox transitions. Neither inspected assembly supplies the chat PDF-card
  rendering predicate. Therefore the exact live web thumbnail/PDF generation
  policy has **not** been established by this package's native code. Heya's
  local first-page renderer is an independent implementation, not a port or
  claim about WhatsApp's internal renderer.
- `MediaDownloadManager.OnDownloadStarting` state machine at `0x7efa0` takes
  a WebView2 deferral, marks the download handled, and checks whether it is an
  expected media download. Expected downloads get the prepared path; manual
  downloads call `MediaSaveFilePicker.SelectSaveFilePathAsync`. Cancelling that
  picker cancels the operation. Completion uses download state notifications
  (with an additional delayed completion check).
- `PrepareForMediaFileSavingInternal` at `0x806b0` records expected downloads
  and in-progress completion tasks. `WaitTillMediaDownloadCompletesInternal`
  at `0x81f90` awaits the matching task instead of starting another transfer.
- `TrySaveMediaFileFromSharedBufferInternal` at `0x81810` coalesces concurrent
  saves, asks for a suggested destination, asynchronously writes a WebView2
  shared buffer, checks the file hash, applies Windows Mark of the Web and then
  records a completed download in `SqliteMediaFilesStorage`. Buffers and
  in-progress entries are released on completion/failure.
- `TryWriteBufferToFileAsync` at `0x7f6b0` takes ownership of the shared buffer,
  exposes it as a stream, copies asynchronously, and disposes both stream and
  buffer. This is a WebView/native transport boundary, not evidence of a CDN
  chunk size or encryption algorithm.
- `VerifyFileHashAsync` at `0x81c70` rejects a mismatch, deletes the failed file
  and removes the completed-download record. A filename existing on disk is
  not sufficient evidence of successful transfer.
- Incoming Windows shares are registered in `SharesheetManager`. Its resource
  handler at `0x87660` serves only registered `/sharesheet/` entries as read-only
  file streams with MIME/filename metadata, then removes the registration.
  The web layer handles subsequent send/upload work; the native code inspected
  here does not establish the web upload pipeline or image compression settings.

## Heya transfer direction

Use native file dialogs/portals and application-owned transfer state. Capture
chat/message/destination at activation; do not let later selection changes
redirect a save or send. Share concurrent requests for the same media, provide
cancellation and progress, verify successful completion before publishing the
final file, and retain a clear failure/retry state. Stage outgoing attachments
independently of the composer; persist outbox metadata before clearing them.
Generate thumbnails and image variants away from the Slint event loop.

The pinned whatsapp-rust APIs already provide the relevant protocol primitives:

- `src/download.rs`: `DownloadParams::encrypted` and
  `Client::download_to_writer`; validation, authenticated decryption, hashes,
  route retry and writer errors. Heya currently wraps this with a bounded writer
  and cancellation. A file-transfer sink can stream to a private partial file
  and publish a destination only after the SDK confirms success.
- `src/upload.rs:427`: `Client::upload` encrypts in background work, supports
  media connection refresh and host failover, and returns path/URL, key, hashes,
  length and media-key timestamp. Appropriate for bounded photo variants.
- `src/upload.rs:491`: `Client::upload_stream` accepts already encrypted media
  plus `EncryptedMediaInfo`; the caller owns the encrypted backing storage.
  Use `wacore::upload::encrypt_media_streaming` for larger files. The pinned
  upload retry flow checks existing/partial uploads at 5 MiB and can resume from
  a server offset. This is SDK source evidence, not a measured desktop behavior.
- Upload is separate from sending an `ImageMessage` or `DocumentMessage` with
  the resulting metadata. The outbox must keep that distinction and must not
  automatically retry a send that may already have reached the network.

No new SDK patch is required for the PDF fallback: ordinary document download
already exists. The earlier additive document-thumbnail patch stays isolated in
`patches/` for upstream review. The preview task did not implement downloads-to-user-files or outgoing attachment
messages. The subsequent explicit download implementation is described below.


## Explicit attachment downloads implemented later on 16 October

Heya now connects the attachment menu to a native Linux Save As dialog. Like the
inspected Windows flow, cancelling the picker cancels the operation, concurrent
starts do not duplicate it, and final output is published only after successful
verification. Heya streams directly through the pinned SDK instead of introducing
a WebView/shared-buffer boundary. No additional SDK patch was required.

Application-owned state captures the source chat/message when activated, supports
Cancel/Retry, and remains independent of navigation. A private same-directory
staging file protects an existing destination until authentication, length checks
and final source revalidation succeed. Progress updates at most ten times per
second and does not rebuild the timeline. Only one manual save runs at a time;
its transport permit is separate from automatic previews/audio.

This is an independent Linux implementation inspired by the audited behavior.
It does not claim Windows Mark of the Web, a persistent completed-download
registry, outgoing attachment support, or exact replication of the web layer.
See [downloads](downloads.md) for limits and test evidence.


## Outgoing files/images audit and Heya implementation

A further IDA pass of the same supplied Root assembly found
`SharesheetManager.RegisterFiles` at `0x531e0`: it clears the old unconsumed
collection and registers each selected StorageFile under a fresh GUID path.
`HandleSharesheetWebResourceRequestedAsync.MoveNext` at `0x87660` accepts only
registered `/sharesheet/` paths, obtains a WebView deferral, awaits
`StorageFile.OpenAsync(FileAccessMode.Read)`, and supplies the stream with
Content-Type and Content-Disposition/filename headers. It removes the consumed
registration and completes the deferral on exit. This is evidence for explicit
file registration, asynchronous read-only streaming, metadata and cleanup at the
native/web boundary. It does not establish the web editor's compression, caption
limits, upload scheduling or exact preview layout. No native OpenFile/editor
implementation was established in the inspected Root methods.

The supplied Material3 `Colors.g.xaml` exposes attachment colors
`AttachmentTypePhotosColorColor` (`#007BFC`) and
`AttachmentTypeDocumentsColorColor` (`#7F66FF`), plus media-editor caption colors.
Heya uses the two attachment colors in its Photos/Documents menu and implements
its own native Slint review surface; it does not port the WebView/editor.

Heya now connects + → Photos/Documents to the Linux OpenFile portal, prepares
encrypted immutable temporary data, and reviews the captured recipient/files
and individual captions before Send. The SDK's public streaming encryptor,
UploadSource/upload_stream and ImageMessage/DocumentMessage primitives cover the
transport; no additional SDK change was needed. The encrypted archive receives
the prepared envelope with a stable staging ID before the ordinary outbox can
send it. Retries cannot reinsert or reset an already attempted row. See
[sending attachments](attachments.md) for independent product limits and test
evidence; none of those limits is claimed to have been extracted from WhatsApp.

## Browser inspection for future reference behavior

The owner's suggested workflow is to inspect WhatsApp Web in a separate Chromium
window for cases that need live UI or request-flow evidence. Chromium and Chrome
are installed locally. DevTools can inspect request/response metadata, timings
and WebSocket messages ([Chrome documentation](https://developer.chrome.com/docs/devtools/network/reference/)).
The web client's encrypted transport/media bodies do not directly expose their
application plaintext in an ordinary network capture; inspecting decoded
client-side data is a separate investigation. A browser reference also does not
establish a Windows native bridge's behavior or guarantee identical feature flags.

Use a separate browser profile, deliberate manual account actions and narrowly
scoped inspection. Record redacted behavior findings, keeping account credentials,
QR payloads, private content and raw authenticated traffic out of logs/fixtures.
Close the investigation's browser/monitor when done; no persistent traffic watcher
is part of Heya. No browser/account capture was started to diagnose the false
upload cancellation, which was reproduced locally with synthetic tests.
