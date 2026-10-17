# Protocol patches queued for upstream

Runtime SDK base: `oxidezap/whatsapp-rust` at
`0dc38af07632f48f0cf6c66df285f1643fc22b95`. The reference checkout is unchanged.

| Patch | API gap | Scope | Upstream status |
| --- | --- | --- | --- |
| [document-thumbnail.patch](document-thumbnail.patch) | `DocumentMessage` has a separate thumbnail path and hashes, but `MediaType` cannot represent its HKDF context. | Adds `DocumentThumbnail`, its crypto/MMS/path mappings, and synthetic tests in `wacore/src/download.rs`. | Local; not submitted. Live interoperability still to verify. |

The additive enum variant is appended to preserve existing numeric casts. Heya
adapts `thumbnail_direct_path`, `thumbnail_sha256`, `thumbnail_enc_sha256` and the
parent `media_key` through public `DownloadParams`; the SDK still performs route
validation, authenticated streaming, decryption and both hash checks. No PDF
parser, custom crypto, HTTP bypass or cache tuning is introduced.

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
patching build script. Only `src/download.rs` has runtime source changes.
Its generated Cargo manifest expands workspace dependencies; SDK sibling crates
still reference the exact base Git revision. Optional upstream features are kept;
Heya enables none of the VoIP code.

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
run its download tests and include live interoperability evidence. Do not bundle
Heya's cache, UI or storage code into the upstream proposal.

Validation on 2026-10-16: the patched crate's 40 `download::tests` passed,
including the new crypto test and the old media-type numeric-cast regression.
The test dependency resolution is saved in `vendor/wacore/Cargo.lock`; it does
not change Heya's runtime dependency versions. Heya separately tests metadata
selection and bounds through its protocol adapter. Live CDN compatibility is
not established by these synthetic tests.

## Text shaping patch

[parley-color-emoji.patch](parley-color-emoji.patch) is separate from the protocol
SDK. It applies to the exact crates.io Parley 0.11.1 source in `vendor/parley`.
The platform emoji family precedes monochrome text fonts for emoji clusters;
ordinary digits/#/* remain text unless part of a keycap, and VS15 retains text
presentation. Source checksum, patch hash, tests and removal/upstreaming notes
are in [docs/color-emoji.md](../docs/color-emoji.md) and `prep/upstreams.json`.
This patch is local and has not been submitted upstream.
