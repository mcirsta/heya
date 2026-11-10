# Protocol patches queued for upstream

Runtime SDK base: `oxidezap/whatsapp-rust` at
`0dc38af07632f48f0cf6c66df285f1643fc22b95`. The reference checkout is unchanged.

| Patch | API gap | Scope | Upstream status |
| --- | --- | --- | --- |
| [ptv-view-once.patch](ptv-view-once.patch) | `MessageExt::is_view_once` omits the inline `ptvMessage.viewOnce` flag. | Recognize round-video privacy flags alongside ordinary videos, with a synthetic regression. | Local; not submitted. |
| [album-child.patch](album-child.patch) | Album children use `associatedChildMessage`, but public message/media helpers leave it wrapped and miss nested view-once. | Add this wrapper to borrowed/consuming base-message helpers and privacy detection, with synthetic regressions. | Local; not submitted. |
| [document-thumbnail.patch](document-thumbnail.patch) | `DocumentMessage` has a separate thumbnail path and hashes, but `MediaType` cannot represent its HKDF context. | Adds `DocumentThumbnail`, its crypto/MMS/path mappings, and synthetic tests in `wacore/src/download.rs`. | Local; not submitted. Live interoperability still to verify. |
| [video-preview-core.patch](video-preview-core.patch) | Video posters have their own thumbnail crypto type; prefix decoding isn't a full authenticated download. | Adds `VideoThumbnail`, explicitly preview-only block decoding and crypto regressions to `wacore`. | Local; not submitted. |
| [video-preview-client.patch](video-preview-client.patch) | No public bounded initial-video download helper. | Adds `Client::download_video_preview_to_writer`, using existing route refresh/host retry, strict range/read caps, cancellation-aware writers and synthetic HTTP tests. | Local; not submitted. |
| [call-log-writer.patch](call-log-writer.patch) | The SDK reads `call_log` app-state mutations but cannot write one. The registry schema declares only the `call_log` literal, so `send_app_state_action` would put every call on one index, and the raw-index writer is crate-private. | Adds `Client::send_call_log`, which writes WA Web's `["call_log", creator, callId, fromMe]` index and a `callLogAction` value, with a wire-capture round trip through the existing reader. | Local; not submitted. Live interoperability still to verify. |

The video preview client extension vendors the same pinned SDK's root library in
`vendor/whatsapp-rust`; its only runtime source changes are `src/download.rs` and
the new `src/download_video_preview.rs`, plus the call-log writer in
`src/features/call_log.rs`. All other SDK siblings remain at the exact Git
revision above. No calling, send, authentication or session behavior is changed;
the writer adds one app-state mutation through the existing patch path. The
vendored source retains upstream MIT notices and optional features.
The root manifest is mechanically flattened for use outside its original workspace;
the separate SDK lockfile fixes test-only dependencies too.

The [video preview audit](../docs/video-previews.md) records the independently
implemented Web selection/range behavior and the distinction between untrusted
preview prefixes and authenticated originals. Reproduce `wacore` with the existing
vendoring script, then reproduce the SDK client with:

```sh
python3 scripts/vendor-video-preview.py --source .references/whatsapp-rust
CARGO_TARGET_DIR=target cargo test --manifest-path vendor/whatsapp-rust/Cargo.toml --locked --no-default-features --features tokio-runtime,tokio-transport,sqlite-storage,ureq-client,tokio-native,voip --lib download::video_preview::tests
```

Before upstream submission, apply `video-preview-core.patch` with
`git apply --directory=wacore` at the upstream workspace root and apply `video-preview-client.patch`
with `git apply` at that same root. Keep both changes separately reviewable from
Heya's cache, native decoder and UI integration. The current tests establish
synthetic protocol behavior, not live CDN compatibility.

The additive enum variants are appended to preserve existing numeric casts. Heya
adapts `thumbnail_direct_path`, `thumbnail_sha256`, `thumbnail_enc_sha256` and the
parent `media_key` through public `DownloadParams`; the SDK still performs route
validation, authenticated streaming, decryption and both hash checks. No PDF
parser, custom crypto, HTTP bypass or cache tuning is introduced. Outgoing PDF
thumbnails use the same variant through the public `Client::upload` with
`UploadOptions::with_media_key`, so the SDK encrypts under the document's key and
uploads to `/mms/thumbnail-document`.

The HKDF context is `WhatsApp Document Thumbnail Keys`, matching the
[Baileys media-type mapping](https://github.com/WhiskeySockets/Baileys/blob/master/src/Defaults/index.ts).
The SDK test includes an independently generated RFC 5869 SHA-256 known answer,
authenticated round trip, rejection under the original document context, and
tamper rejection. The existing exhaustive media-route tests include the new type.
All test bytes are synthetic.

## Reproduction and review

`vendor/wacore` contains the upstream crate's sources, build script, test/bench
files, license and Clippy configuration. It is a source snapshot so ordinary
`cargo build --locked` works without modifying Cargo's Git cache or running a
patching build script. Runtime changes are confined to `src/download.rs` and `src/proto_helpers.rs`.
Its generated Cargo manifest expands workspace dependencies; SDK sibling crates
still reference the exact base Git revision. Optional upstream features are kept;
Heya uses the SDK's public VoIP feature without patching its calling code.

To regenerate from the clean pinned reference checkout:

```sh
python3 scripts/vendor-document-preview.py
cargo test --locked document_preview
CARGO_TARGET_DIR=target/sdk-tests cargo test --locked --manifest-path vendor/wacore/Cargo.toml --lib download::tests
```

The script validates the base revision and rejects tracked reference changes.
`prep/upstreams.json` records the patch and its SHA-256. Keep this queue updated
for every future protocol patch, including rationale, tests, base SHA, submission
URL and eventual removal when an upstream revision includes it. Before submitting,
apply the patch inside the upstream crate (`git apply --directory=wacore ...`),
run its affected download/proto-helper tests and include live interoperability evidence. Do not bundle
Heya's cache, UI or storage code into the upstream proposal.

Validation on 2026-10-16: the patched crate's 40 `download::tests` passed,
including the new crypto test and the old media-type numeric-cast regression.
The test dependency resolution is saved in `vendor/wacore/Cargo.lock`; it does
not change Heya's runtime dependency versions. Heya separately tests metadata
selection and bounds through its protocol adapter. Live CDN compatibility is
not established by these synthetic tests.

## Call-log writer

[call-log-writer.patch](call-log-writer.patch) applies to the pinned
`src/features/call_log.rs` and is applied after the video-preview client patch by
`scripts/vendor-video-preview.py`. WA Web publishes every call it handles as a
`call_log` mutation in the `regular` collection, so the phone and other
companions list it. The index is `["call_log", callCreatorJid, callId,
fromMe ? "1" : "0"]`, the encoding the SDK's reader documents and parses. The
generated schema registry declares only the literal, and the raw-index writer is
crate-private. A Heya-side workaround would have to re-implement syncd encoding.
`Client::send_call_log` encodes the caller's `CallLogRecord` under schema
version 1 and sends it as a `Set` through the existing patch path. Heya chooses
the result codes from WA Web's VoIP mapping (`WAWebVoipWaCallEnums`) and the
literal `isIncoming`, as the phone writes for its own calls.

Run the SDK tests with:

```sh
CARGO_TARGET_DIR=target cargo test --manifest-path vendor/whatsapp-rust/Cargo.toml --locked --no-default-features --features tokio-runtime,tokio-transport,sqlite-storage,ureq-client,tokio-native,voip --lib features::call_log
```

The wire-capture test decodes the sent patch with the test sync key and feeds it
back through the reader: same call, creator and record, read as placed by this
account. An empty call ID is refused before anything is sent. Whether the phone
lists Heya's calls has not been observed yet. Remove the patch when an upstream
revision offers a call-log writer.

## Text shaping patch

[parley-color-emoji.patch](parley-color-emoji.patch) is separate from the protocol
SDK. It applies to the exact crates.io Parley 0.11.1 source in `vendor/parley`.
The platform emoji family precedes monochrome text fonts for emoji clusters;
ordinary digits/#/* remain text unless part of a keycap, and VS15 retains text
presentation. Source checksum, patch hash, tests and removal/upstreaming notes
are in [docs/color-emoji.md](../docs/color-emoji.md) and `prep/upstreams.json`.
This patch is local and has not been submitted upstream.

## Linux text surface policy

[slint-skia-text-coverage.patch](slint-skia-text-coverage.patch) applies only to
Slint's pinned `i-slint-renderer-skia` 1.19.0 development snapshot. Linux surfaces use unmodified
grayscale coverage; other platforms keep their original defaults. This adds no
Skia features or rendering passes. Base checksum, reproduction, isolation and
validation are in [docs/text-rendering.md](../docs/text-rendering.md); the patch
hash is in `prep/upstreams.json`. Local only, not submitted upstream.

## Native clipboard URI lists

`vendor/arboard` is the published arboard 3.6.1 crate, including its MIT/Apache
notices. [arboard-uri-list.patch](arboard-uri-list.patch) is the only source
change: parse CRLF before percent decoding, recognize the local `localhost`
authority, and reject remote authorities instead of treating them as relative
paths. This fixes file-manager copies without rewriting filenames in Heya.
The crate-level regression preserves encoded carriage returns inside names.
The private Xvfb clipboard check also reads a CRLF URI offer from `xclip`.
Patch status: local, not submitted upstream. Remove it when an upstream release
includes the fix. To reproduce, unpack the crates.io `arboard-3.6.1` source and
apply the patch with `patch -p1`; no binaries or generated files are required.

## Slint opacity layers

[slint-skia-opacity-bounds.patch](slint-skia-opacity-bounds.patch) limits ordinary
opacity-layer allocation using Slint's existing child-bounds calculation, rather
than allocating for the entire current clip. It preserves group compositing,
overflow and transforms. Heya's thumbnail styling is unchanged. The patch applies
to the pinned renderer after the text-coverage patch; the vendoring script reads
both entries from `prep/upstreams.json`. See [the resize investigation](../docs/resize-performance.md)
for synthetic measurements, renderer regressions and validation limits. The
matching upstream issue is [13834](https://github.com/slint-ui/slint/issues/13834).
The adaptation for current master is submitted as
[draft PR 14307](https://github.com/slint-ui/slint/pull/14307), retaining master's
backdrop-blur behavior. Remove the local backport when Heya's pinned upstream
revision includes the fix. The draft awaits Slint's contributor agreement and
review.

## Poll creation send options

`poll-creation-options.patch` exposes the existing SDK E2E creation-send path
as `Client::send_creation_with_options`. The normal poll facade allocates its
own message ID, while Heya's durable outbox must retain its already-saved ID.
The new method accepts the existing `SendOptions` and returns the actual PN/LID
creator identity captured by the send implementation, needed for subsequent
poll vote encryption. Existing convenience APIs delegate with default options.
The patch includes a synthetic transport test checking the caller's ID in the
actual wire envelope and the returned sender namespace. Base revision, hash,
status and reproduction are tracked in `prep/upstreams.json` and
`scripts/vendor-video-preview.py`. No change to the poll cipher is needed.

## X11 window-manager fullscreen state

[winit-x11-fullscreen.patch](winit-x11-fullscreen.patch) patches only the
`winit-x11` 0.31.0-beta.3 backend, from registry archive SHA-256
`bb5746cd907e514b7895491a1c6f7d0cb70077f9e16928cd74ae6f987ffd96aa`
(upstream `7d20408d33210c93bf036335b88ee083ca563a90`). Apache-2.0 notices are
retained in `vendor/winit-x11/LICENSE`. Status: local, not submitted.

The unpatched backend only changes its fullscreen cache when the application
calls `set_fullscreen`; a window-manager shortcut/menu can enter fullscreen while
`Window::fullscreen()` still returns `None`. Slint then reports a normal resize,
so saving window state can replace the normal dimensions with fullscreen bounds.

The patch reads native `_NET_WM_STATE` only when that property changes, updates
borderless fullscreen's existing cache, and notifies consumers through the
existing resize event. It does not poll, query on ordinary resize/render, or
change exclusive video-mode ownership. Heya uses borderless fullscreen. The real
Xwayland restart regression in `scripts/window-state-check.py` reproduces the
incorrect stored normal bounds before this patch; Wayland uses its unchanged
native backend.

Reproduce from the pinned registry archive (URL/checksums also in
`prep/upstreams.json`):

```sh
python3 scripts/vendor-winit-x11.py --archive /path/to/winit-x11-0.31.0-beta.3.crate
cargo build --release --locked
HEYA_TEST_RENDERER=winit-skia-opengl scripts/ui-offscreen.sh --window-state-check
```

The source snapshot contains only the normalized manifest, source, README and
license. No build outputs or registry cache metadata are vendored. This small
backend fix is kept separate from Heya's persistence logic for upstream review.
