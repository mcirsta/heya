# Linked-device read synchronization

The previous build only cleared Heya's archive badge. The protocol adapter now
uses the pinned SDK's public `chat_actions().mark_chat_as_read` API to synchronize
that state with WhatsApp's other linked clients. This is a private app-state
update, separate from sender-facing read receipts/blue ticks. It needs no SDK
patch and does not enable or change receipt privacy settings.

A read starts after 200 ms at the latest end of the displayed conversation, with
the window active and unoccluded. Minimized windows, settings, dialogs, the photo
viewer, a hidden narrow-layout conversation, and browsing older history suppress
it. Focus/occlusion come from the existing winit event adapter; toolkit minimized
state supplies the other window gate. One window timer rechecks visibility and
navigation revision before sending a small command to the service. Rendering
and scrolling perform no SQL or protocol I/O.

The service no longer marks chats read when constructing snapshots. It checks the
current conversation/revision and records the displayed row boundary in a SQLCipher
transaction with the local unread change. Newer arrivals retain their local badge,
including arrivals in the same second. Schema 9 adds `outgoing_reads`, one position
per chat, plus an index for pending jobs. Previously cleared badges aren't
broadcast during migration: revisiting a conversation creates the first update.

One supervised background worker waits for a verified connected session, then
sends one update at a time. Failures/timeouts back off for 30 seconds without
archive bursts bypassing the delay. The queue survives shutdown/restart. A stale
completion cannot remove a newer read or work changed by PN/LID merging. A newer
manual unread mark received from another device cancels an older queued read.
Successful positions remain for deduplication; repeated views don't rewrite or
resend them. A confirmed outgoing message advances the same queue even when the
local badge is already zero. Queued/failed messages do not advance the remote
range. This uses the existing ACK/receipt path, with no extra visibility state,
service, timer or schema. The server ACK supplies the actual send time for offline
queued messages. The range includes the message key, author and from-me flag when
that original identity remains unambiguous; expired/deleted bodies retain their
original timestamp-only fallback. Repeated ACKs/revisits deduplicate by position,
and late acknowledgments cannot clear newer incoming messages or a newer manual
unread mark. Old outgoing history is not broadcast during import.
SDK app-state ranges use message timestamps in seconds, matching the
pinned ZapFast path. Incoming keyed app-state echoes preserve the exact stored
message boundary so a newer incoming row in the same second remains unread.
Remote clients determine their own conflict resolution.

Reference inspection: `.references/zapfast/src/backend/worker.rs`, `mark_read`
and `pump_read_sync`, at the revision recorded in `prep/upstreams.json`.
Heya's queue, visibility rules and worker are implemented independently. The
SDK implementation is in `src/features/chat_actions.rs` of the pinned
whatsapp-rust checkout.

Validation uses synthetic encrypted archives, a controllable transport, and
Slint's testing backend. It covers atomic rollback, migration from schema 8,
restart, coalescing, late arrivals, stale acknowledgments, identity merging,
manual unread marks, offline gating, timeout/backoff/shutdown, and visibility
and navigation gates. These checks do not establish real-account propagation
or desktop focus/compositor compatibility. No automated test sends read updates
to the owner's account.

For deliberate owner verification, run the updated normal build, open a chat
that has unread messages in the phone/ZapFast, and view its latest messages while
Heya has focus. Check the badge in the other client. Repeat after reading offline
and reconnecting. Leave Heya unfocused for another incoming message to check that
it remains unread until the conversation is actively viewed.

## Validation recorded 2026-10-16

`cargo test --locked` passed 104 unit/service/toolkit tests and one encrypted
protocol-storage integration test; the deliberate real-keyring test is ignored.
Formatting, all-target Clippy with warnings denied, the media-disabled build
check and the optimized default build passed. Logs are under
`.prep/validation/read-sync-20261016/`.

Two before/after runs used the optimized synthetic 10,000-message card benchmark
via `scripts/ui-offscreen.sh` and `winit-skia-opengl`. The timeline scroll layout
p95 stayed at 0.16–0.18 ms; render p95 was 16.05–16.54 ms before and 16.51–16.52 ms
after. No material scrolling change was observed. Resize/typing render samples
were more variable (typing p95 12.46–14.14 ms before, 15.78–15.97 ms after), so
these runs do not establish identical performance in every interaction. Xvfb
reported unavailable DRI3 acceleration. These are offscreen application/render
samples, not native Vulkan, compositor animation, network latency or account
interoperability measurements. The final subsequent change only suppresses
redundant background archive/network work; it does not change the measured UI.

The subsequent outgoing-message correction passed 131 application/service/UI
tests and one protocol-storage integration test, including confirmed sends,
offline queue time versus ACK time, restart, duplicate ACKs, manual unread,
same-second arrivals and keyed app-state echoes. The deliberate keyring test
remains opt-in. The owner then sent a message manually and confirmed that the
chat stayed read in ZapFast/the other WhatsApp client. This confirms that live
outgoing case; automated checks did not send account messages. Logs are under
`.prep/validation/outgoing-read-20261016/`.
