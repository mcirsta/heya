# Heya upstream audit

Reviewed on 13 October 2026. The source snapshots below, rather than a moving
README or the latest branch, define the initial reference. The machine-readable
record is [upstreams.json](../prep/upstreams.json).

| Project | Reference | Finding |
| --- | --- | --- |
| whatsapp-rust | [`0dc38af`](https://github.com/oxidezap/whatsapp-rust/tree/0dc38af07632f48f0cf6c66df285f1643fc22b95) | Package 0.7.0; workspace Rust floor 1.94. Native features can be selected explicitly. |
| ZapFast | [`c73ce57`](https://github.com/crmne/zapfast/tree/c73ce57522d28eff4da1a5cc9e3b6de0f27b0de0) | Package 0.19.0, development HEAD; Rust floor 1.98. Already uses whatsapp-rust. |
| Slint | [`1.19.0 development / a5a124b`](https://github.com/slint-ui/slint/tree/a5a124bf80961966e875dc91d8d04271ec5885d8) | Pinned winit 0.31 branch for native Wayland drops. Native accessibility bridge disabled upstream. All Slint crates use the same revision. |

## Protocol integration

ZapFast pins the **same protocol commit** through the `crmne/whatsapp-rust` fork.
Heya can use the user-selected `oxidezap/whatsapp-rust` URL at that revision.
Avoid replacing the pin with the crates.io `0.7` range: equal package version
strings do not establish equal source or feature behavior. The library checkout
has a nightly toolchain file, while its manifest declares stable compatibility;
Heya should validate its own stable build with only the required features.
See both [protocol manifest](https://github.com/oxidezap/whatsapp-rust/blob/0dc38af07632f48f0cf6c66df285f1643fc22b95/Cargo.toml)
and [ZapFast manifest](https://github.com/crmne/zapfast/blob/c73ce57522d28eff4da1a5cc9e3b6de0f27b0de0/Cargo.toml).

The adapter should start from `Bot::builder()`, a persistent `SqliteStore`,
filtered event subscriptions, and the typed client methods. QR/code pairing,
session resume, direct/group messages, receipts, media and history are available
in the source. These still need end-to-end application tests. The message event
is `Event::Messages(MessageBatch)` at this pin; do not implement against older
examples that assume an individual message variant.
Source: [bot.rs](https://github.com/oxidezap/whatsapp-rust/blob/0dc38af07632f48f0cf6c66df285f1643fc22b95/src/bot.rs),
[events.rs](https://github.com/oxidezap/whatsapp-rust/blob/0dc38af07632f48f0cf6c66df285f1643fc22b95/wacore/src/types/events.rs).

The protocol database is not Heya's searchable message archive. Add application
storage, idempotent ingestion and outgoing-message reconciliation. The current
library offers an inbound durability hook before acknowledgment and a separate
history capture hook. Register both deliberately; a normal event callback does
not provide the same durability. History receipt suppression does not guarantee
phone redelivery, and some message paths bypass the inbound hook.
Source: [durability_hook.rs](https://github.com/oxidezap/whatsapp-rust/blob/0dc38af07632f48f0cf6c66df285f1643fc22b95/src/types/durability_hook.rs).

SQLCipher integration is possible through the store's per-connection init hook.
It requires an encrypted SQLite build and keying every pooled connection before
pragmas or migrations. Plain `SqliteStore::open` is not evidence of encryption.
Source: [sqlite_store.rs](https://github.com/oxidezap/whatsapp-rust/blob/0dc38af07632f48f0cf6c66df285f1643fc22b95/storages/sqlite-storage/src/sqlite_store.rs).

### Suitability as Heya's backend

Source review supports using this library as the protocol engine for a native
Slint client. No GUI-enabling patch has been identified. The `Bot` name describes
a convenience facade: `Bot::spawn` starts background execution and returns an
owned handle with access to `Arc<Client>`. Heya can issue commands independently
of incoming event callbacks. `BotHandle::shutdown` flushes protocol state and
awaits the run loop, but callback completion, detached history work and release
of storage ownership have separate boundaries. Heya must own its application
tasks and shutdown sequence.
Source: [bot lifecycle](https://github.com/oxidezap/whatsapp-rust/blob/0dc38af07632f48f0cf6c66df285f1643fc22b95/src/bot.rs).

The important integration constraint is event delivery. The default asynchronous
adapter queues 256 events and permits 16 callbacks concurrently; full queues drop
new events and callbacks may finish out of order. Ordered delivery still drops on
overflow. Raw synchronous handlers and delivery counters are available, but
handlers on the dispatch path must stay fast. Use durability hooks for supported
messages/history and verify capture/recovery of critical event-only updates
separately. Increasing queue capacity is not a persistence guarantee.
Source: [event delivery](https://github.com/oxidezap/whatsapp-rust/blob/0dc38af07632f48f0cf6c66df285f1643fc22b95/src/bot/event_delivery.rs).

`SendOptions::with_message_id` lets Heya persist an outbox entry and its protocol
ID before sending, then reuse the ID on retry. This is useful for reconciliation;
it does not by itself promise exactly-once delivery after an ambiguous failure.
Source: [send API](https://github.com/oxidezap/whatsapp-rust/blob/0dc38af07632f48f0cf6c66df285f1643fc22b95/src/send/mod.rs).

Media APIs support more than in-memory byte buffers. `download_to_writer` can
stream authenticated decryption into a truncatable sink with a streaming HTTP
adapter. The sink must remain private until successful verification; cancellation
cannot guarantee cleanup. `upload_stream` accepts a reopenable source of already
encrypted media, prepared with the streaming encryption helper. Heya still owns
staging files, progress presentation, cancellation policy, thumbnails and playback.
The presence of these APIs does not prove immediate cancellation of blocking I/O
or progressive playback. Verify those behaviors before promising them in the UI.
Sources: [download API](https://github.com/oxidezap/whatsapp-rust/blob/0dc38af07632f48f0cf6c66df285f1643fc22b95/src/download.rs),
[upload API](https://github.com/oxidezap/whatsapp-rust/blob/0dc38af07632f48f0cf6c66df285f1643fc22b95/src/upload.rs).

Start with the pinned library unchanged. The user has authorized targeted changes
if a concrete need appears. Possible investigation areas are durable capture of
event-only changes, transfer cancellation/progress and shutdown ownership; these
are validation questions, not confirmed missing APIs or a reason to fork now.
Message archive/search, drafts, outbox, Slint models, clipboard behavior and video
playback belong to Heya regardless of whether the protocol library is patched.

## ZapFast visual reference

ZapFast is used for appearance only. Its existing features and implementation
are not Heya's behavioral baseline. Use screenshots and presentation source to
recover colors, spacing, proportions and component appearance, then implement
Heya's own interactions and application architecture in Rust and Slint.

| Source path in ZapFast | Use in Heya preparation |
| --- | --- |
| `src/theme.rs`, `src/ui/chats.rs` | Color roles, chat rows, responsive sidebar and sizing |
| `src/ui/conversation.rs`, `src/ui/pane.rs` | Bubble, composer and search-pane appearance |
| `src/ui/login.rs`, `src/ui/settings.rs` | Pairing and settings appearance |
| `src/ui/image_preview.rs`, `src/ui/video_preview.rs` | Viewer appearance |
| `DEMO.md`, `docs/screenshot*.png` | Offline visual fixtures and comparison views |

These are source navigation hints relative to the pinned repository. Do not
reuse ZapFast's application, model, backend, archive, media or clipboard logic.
Its egui/fastframe dependencies and patches are not part of Heya's dependency
plan. Functional requirements come from Heya's own UI specification.

## Slint feasibility findings

- `ListView` instantiates visible rows. Variable-height chat bubbles, prepending
  history and retaining the scroll anchor still need a representative prototype.
- `TextEdit` supports read-only text selection and copy/paste functions. This
  does not establish rich-text selection across different message components.
- The pinned docs describe `data-transfer` and `DropArea` for text, images and
  file paths. Check the chosen backend's actual clipboard and external-drop
  support before adding a second clipboard library.
- The GStreamer example demonstrates both an image-buffer sink and Linux EGL
  integration. It is an integration example, not a complete chat video player.
  Heya needs bounded frame delivery, correct row strides, error handling and
  shutdown when the window disappears.

Sources: [ListView](https://github.com/slint-ui/slint/blob/372cf0ee5577c3dfec309a45e7b778ba4e81b734/docs/astro/src/content/docs/reference/std-widgets/views/listview.mdx),
[TextEdit](https://github.com/slint-ui/slint/blob/372cf0ee5577c3dfec309a45e7b778ba4e81b734/docs/astro/src/content/docs/reference/std-widgets/views/textedit.mdx),
[drag and drop](https://github.com/slint-ui/slint/blob/372cf0ee5577c3dfec309a45e7b778ba4e81b734/docs/astro/src/content/docs/guide/development/drag-and-drop.mdx),
[GStreamer player](https://github.com/slint-ui/slint/tree/372cf0ee5577c3dfec309a45e7b778ba4e81b734/examples/gstreamer-player).

Slint must own the main event loop. Tokio workers communicate through
`invoke_from_event_loop` or a weak component handle's event-loop upgrade; Slint
models stay on the UI thread. Source: [Rust API](https://docs.rs/slint/1.18.1/slint/).

## Scope of verification

Preparation inspected the pinned source, manifests, license notices, and
ZapFast's supplied screenshots. The first implementation builds Slint and the
pinned protocol dependency together on the installed stable Rust toolchain.
Offline tests exercise the protocol store's SQLCipher connection hook, wrong-key
rejection and full-text search. The synthetic Slint demo has been launched on
Linux. Heya has not been paired or exercised with live traffic; those remain
separate roadmap acceptance gates. See [development evidence](development.md).
