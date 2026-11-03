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

## WhatsApp Web document previews — 3 November 2026

Question: does WhatsApp produce document (especially PDF) chat previews in a way
Heya is missing, and is Heya's local first-page renderer necessary?

Source: the JavaScript that `https://web.whatsapp.com/` loads without a login.
The app's built-in browser opened the page. Its user agent gets WhatsApp's
client-side "Chrome 100+" notice, but the page still loaded nine
`static.whatsapp.net/rsrc.php` bundles (about 13 MB). Those exact URLs were then
fetched with plain `curl` into a scratch directory outside the repository and
read as text only; nothing was executed, no account was used and no message was
sent or received. Behaviour below is described from de-minified reading; no
WhatsApp code is reproduced. Offsets are byte offsets of each module's
`__d("…")` definition.

| Label | rsrc file (prefix) | Bytes | SHA-256 |
| --- | --- | --- | --- |
| B1 | `UpzmlOYfZtG.js` | 303973 | `83ef176e68464d45f60a63fddaa2e753035ceae3b8a17f6bbe380fceac076356` |
| B2 | `RQX8HtPYDNkENA-HSR9WTIqy…` | 555220 | `b7ac71dc876a7039f027df49f4a69dd8dfb1588a666d1912fff6bf4858e0515d` |
| B3 | `PAl4L2h_M1g.js` | 288746 | `3d489c646a24a37cb138f9780a88507df76a7784a4561940f9a7397e3f4855ba` |
| B4 | `6dSuDXZks19.js` | 103059 | `c6d684a423ba6f1e4d5d0da4d8878cfb82ceba8cf7d16e1c657af0f3a76d7cc5` |
| B5 | `zdBFXGM3kn4FllrhzqMqikts…` | 1121722 | `e576f472338d122d8af512cfff5f8def44f753a299f66ed6c6ce4fdd2b76e6cc` |
| B6 | `DhiO42nktKwrn7FOwNdXruMq…` | 129666 | `52965ff13adbb8753c84a21e009bd79053614c51eda0e6ed5af5d50c6e110938` |
| B7 | `iEnTn-tczCnb2OVAl7obmDJ0…` | 6030680 | `6d916ab261e657a72a9f5848d5d768abf7803411f27184b734f778f6e6c5f1b1` |
| B8 | `nfMK_RvGZhn.js` | 3279289 | `a77b5afc2ccf23abf922632a6b8da0d479245429825dbb6a088828a9714b5f68` |
| B9 | `svVCnL32geB.js` | 1315750 | `f13c68bf9120a66503c086370e90deba005a2e7ba136d39a1dfa1bca505b9d60` |

Build identifier: `WAWebBuildConstants` (B7 @628690) forms the version as `2.3000.`
plus the server-supplied `SiteData.client_revision`. The notice page did not
populate `SiteData`, so the revision is **not recorded**; the hashes identify
the build. The A/B values quoted below are the client defaults in
`WAWebABPropsConfigs` (B7 @192199); live server values need a logged-in session
and are not known.

### Received documents (questions 1 and 2)

- `WAWebParseDocumentMessageProto` (B7 @835853) keeps `jpegThumbnail` as the
  message's embedded preview. It also stores `thumbnailDirectPath`, both
  thumbnail hashes, `thumbnailWidth`/`thumbnailHeight` and `pageCount` (0 when
  absent).
- Every newly received document is queued for an HD-thumbnail fetch,
  independently of the user's auto-download settings (`WAWebProcessMultipleMsgsAction`,
  B8 @2326343). `WAWebMediaAutoDownloadQueue` (B8 @986162) runs up to four such
  thumbnail fetches at once.
- `WAWebMediaDownloadMmsThumbnail` (B8 @1016019) attempts the fetch only when the
  path and plaintext hash are present. Encrypted chats also need the encrypted
  hash, media key and key timestamp. Documents also need `thumbnailWidth` and
  `thumbnailHeight`; without them the fetch is abandoned. It downloads with type
  `thumbnail-document` using the **document's own media key** and stores the JPEG
  as the message's full-quality preview. The recorded size comes from the
  message's width/height fields, not from decoding the image.
- `getHighestQualityThumbnailUrl` in `WAWebMediaDataUtils` (B8 @1146910) returns
  that HD preview when present and otherwise the embedded `jpegThumbnail`. That is
  the order: HD thumbnail first, then the embedded JPEG.
- Downloading the whole document is a separate auto-download. It requires the
  user's document auto-download preference, a trusted, newly arrived message and
  at most 32 MiB.
- The only captured PDF page renderer, PDF.js (`WAWebPDFUtils`, B8 @1277814), is
  reached only from outgoing-document preparation. No captured receive path
  downloads a document to make a preview.
- With only a tiny `jpegThumbnail` and no HD reference, the HD fetch exits early
  and nothing else is attempted. The preview stays the embedded JPEG. WhatsApp
  Web's own senders produce embedded thumbnails of 1300 bytes or less (see
  below), so tiny embedded images are normal, not a fault.

### Sent documents (question 3)

- Previews are generated only for PDFs (`WAWebDocumentUploadEnrichment`,
  B8 @1274997). Other document types are sent without a generated image.
- PDF.js is loaded on demand with `eval` disabled and renders page 1 at scale 1,
  one canvas pixel per PDF point (A4 is about 595×842). An internal "WebTP"
  renderer (`WAWebTPDocumentEnrichment`, B8 @2101659) is gated off by default
  (`wa_webtp_use_thumbnail_renderer`) and falls back to PDF.js. Rendering times out
  after `wa_webtp_thumbnail_renderer_timeout_ms` (default 3000 ms;
  `WAWebTPThumbnailRenderer`, B8 @1273407). A timeout, failure or
  password-protected file sends the document without a preview.
- HD image: the page is scaled down, never up, to fit a 480-pixel longest edge
  (`DOC_THUMB_MAX_EDGE`, `WAWebMediaConstants` B8 @1120502). An A4 portrait page
  becomes 339×480. It is encoded as JPEG at quality 0.92 (`web_jpeg_quality`
  default 92).
- Embedded `jpegThumbnail`: a JPEG of the page. If it exceeds
  `web_pdf_thumbnail_size_in_bytes` (default 1300), its dimensions are halved and
  it is re-encoded, up to ten attempts (`generateMicroThumb`, `WAWebCanvasUtils`
  B7 @2436534). A typical A4 page therefore ends near 74×105 or 37×52. The upload
  step applies the same 1300-byte bound again.
- `pageCount` is the caller's value or, failing that, the PDF's page count. It
  is written only when finite (`WAWebGenerateDocumentMessageProto`, B8 @351062).
- `WAWebMediaPrepHelpers` (B8 @1093144) always requests a thumbnail upload for
  documents and maps it to `thumbnail-document`. `WAWebMediaUploadMediaWithPrep`
  (B8 @1121237) uploads it alongside the document, encrypted with the
  document's media key. If the document upload ends up with a different key, it
  re-uploads the thumbnail (`wa_web_media_thumbnail_key_match_enabled`); if that
  fails, it sends without one. The message then carries `thumbnailDirectPath`,
  `thumbnailSha256` (plaintext), `thumbnailEncSha256` and the recorded
  width/height. A failed thumbnail never blocks the document send.
- Caveat: read literally, when the message already has an embedded preview (the
  normal PDF case), the upload step sends that embedded-size image to
  `thumbnail-document` and records its dimensions, rather than the 480-pixel
  image. The 480-pixel image is uploaded only when there is no embedded preview,
  or, with the status-HQ gate on (default off), when it is 1300 bytes or smaller.
  This is a static reading of minified code, not an observation on the wire. If
  it is right, a WhatsApp Web sender's "HD" document thumbnail is no larger than
  the embedded one.
- `upload_document_thumb_mms_enabled` and `download_document_thumb_mms_enabled`
  exist in the A/B table (both default false) and in the protocol dictionary.
  No captured code reads them, and the traced paths above do not depend on them.

### Document-thumbnail mapping (question 4)

Confirmed. `WAWebMmsMediaTypes` (B9 @1110203) defines `THUMBNAIL_DOCUMENT` as
`thumbnail-document`. `WAWebCryptoMediaTypeInfo` (B7 @925503) gives it HKDF info
`WhatsApp Document Thumbnail Keys`, separate from `WhatsApp Document Keys`.
`WAWebMmsClientFormatHashUrl` (B7 @1245025) maps it to `/mms/thumbnail-document`.
`WAServerMediaType` (B7 @676156) and `WAWebQueryMediaConnsJob` (B7 @2631712) list
it as a server media type. Both directions use the parent document's media key,
as Heya's adapter does. All of this matches `patches/document-thumbnail.patch`
and is first-party client evidence for it. It is not a live CDN download.

### What the bundles do not establish

- The chat UI. The logged-out page loaded no message-card components, only data
  and media modules, so how a card draws a small embedded image (scaling, blur,
  crop, or an icon instead) is not established.
- Live A/B values, the exact build revision, and anything about phone clients.
  Android and iOS may send larger document thumbnails; nothing here shows it. The
  earlier 51×72 embedded-only PDF came from an unknown client.
- On-the-wire behaviour, including the upload caveat above. No message was sent
  and no CDN request was made.

### Recommendations for Heya

1. **Keep the local first-page renderer, as Heya's own quality choice.** It is
   not something WhatsApp Web does for received documents: Web shows the HD
   thumbnail when one arrives and otherwise the embedded JPEG. Note that
   `document_previews::sharp_enough` requires at least 768×320. A thumbnail
   produced by WhatsApp Web is at most 480 pixels on its longest edge, possibly
   smaller per the caveat above, so it never qualifies. For PDFs, Heya therefore
   always downloads the document (up to 16 MiB) when the sender used WhatsApp
   Web. That remains on demand and bounded, so no change is required. If
   bandwidth matters, an owner-level option is to accept HD thumbnails of at
   least 480 pixels on the longest edge before downloading. For non-PDF documents
   and oversized PDFs, falling back to the embedded JPEG matches WhatsApp Web.
2. **Outgoing PDFs should carry thumbnails.** Match what other clients can rely
   on: an embedded `jpegThumbnail` of at most 1300 bytes (halve the dimensions
   until it fits), and a separate JPEG of at most 480 pixels on the longest edge,
   quality about 0.92. Upload the separate JPEG as `thumbnail-document` with the
   document's media key. Set the five thumbnail fields from the uploaded image's
   actual dimensions, and set `pageCount`. Render with Heya's existing sandboxed
   Poppler/Quartz path away from the event loop, with a short timeout. A failed
   render or thumbnail upload must not block or retry the document send. Other
   document types need no generated image. A larger HD edge would help Heya
   receivers, but how other clients handle one is not established.
   *Implemented 3 November 2026 as described in [attachments](attachments.md);
   not yet verified with a live account or on macOS.*
3. **The SDK patch mapping is confirmed** by first-party web-client code: media
   type, MMS path, HKDF info and use of the parent media key. Before an upstream
   submission, add one read-only live download of a real `thumbnail-document`
   as interoperability evidence, per `patches/README.md`.
