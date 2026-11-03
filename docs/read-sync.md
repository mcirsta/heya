# Read synchronization and sender-facing receipts

## Sender-facing read and played receipts

Settings → Privacy → **Send read receipts** controls both read and voice played
receipts sent by Heya. It defaults to on and is saved locally. Turning it off
also suppresses Heya's group receipts, discards pending sender-facing receipts,
and leaves private linked-device unread synchronization working. Enabling it
does not backfill messages read while it was off. The account-wide read-receipt
setting remains managed in WhatsApp on the phone; Heya does not change it.

A visible read uses the existing active-window/latest-message gate below. In
the same archive transaction, Heya records receipt intent for newly received
incoming messages and known unread history through that exact message boundary,
then advances private
read state. Ingestion, history import, constructing a view and confirming an
outgoing send do not independently queue sender-facing receipts. Older cached
messages already marked read are not backfilled on upgrade. New live arrivals
retain a separate unobserved marker, so a quick outgoing reply clearing the
private badge cannot consume their sender-facing receipt eligibility.

Played intent is recorded only for a current incoming voice-message envelope
after the platform player reports playback progress, or successful completion
of a short playing clip. Downloading, loading, paused seeking, failure before playback,
outgoing audio and ordinary audio files do not count as playing a voice message.
The coordinator uses the same rule for macOS native audio and Linux GStreamer.
Each playback generation is handled once; stored completion deduplicates replay.

Schema 15 adds an encrypted per-message receipt queue. One background sender
batches up to 256 IDs with the same chat, participant and receipt kind. Before
each batch it fetches account privacy through the pinned SDK's public API. A
failed fetch or unknown value leaves the batch pending; it never defaults to
permission to notify the sender. Known-disabled direct-message receipts are
consumed without transmission; private unread synchronization continues through
its separate worker. This explicit guard does not rely solely on the SDK's
concurrently refreshed privacy cache. Group receipt behavior is preserved;
Heya's local off switch suppresses group receipts too. This follows the pinned
SDK's policy, not a claim of verified behavior in every official client.

Offline work waits for reconnect. Failure/timeout backs off for 30 seconds;
disconnect, shutdown and local preference changes cancel in-flight work. An
already transmitted receipt cannot be recalled. Successful SDK transmission
marks the batch complete; a crash between transmission and saving completion
may retry the same IDs. This is not proof of server delivery. Alias merges
preserve intent, stale completions cannot clear a different target, and hidden,
deleted or expired messages are removed from pending work.

Synthetic coverage exercises encrypted reopen, exact boundaries, privacy off
without historical backfill, bounded batches, alias merges, missing account
privacy, offline retry/cancellation and controlled playback progress. Account
interoperability and physical-device checks remain deliberate manual tests;
Linux CI does not establish Linux desktop/device compatibility.

The SDK's played-receipt privacy routing needs a deliberate account check:
WhatsApp's [read-receipt help](https://faq.whatsapp.com/665923838265756/)
describes voice-play exceptions, while the pinned SDK explicitly routes disabled
DM played receipts to `played-self`. This slice conservatively suppresses those
DM played receipts and does not override account privacy to force a public one.

## Private linked-device unread synchronization

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

Visible deletion and expiry placeholders are valid observed read positions.
Viewing one at the latest end clears unread messages through that exact original
row. Later arrivals, even within the same second, stay unread. Revoked bubbles
use the deletion envelope's ID for private synchronization while retaining the
original timestamp and author. Expired bubbles retain their original key.
Sender-facing receipts still exclude deleted, expired and hidden content.

One supervised background worker waits for a verified connected session, then
sends one update at a time. Failures/timeouts back off for 30 seconds without
archive bursts bypassing the delay. The queue survives shutdown/restart. A stale
completion cannot remove a newer read or work changed by PN/LID merging. A newer
manual unread mark received from another device cancels an older queued read.
Successful positions remain for deduplication; repeated views don't rewrite or
resend them. A confirmed outgoing message advances the same queue even when the
local badge is already zero. Queued/failed messages do not advance the remote
range. This uses the existing ACK/receipt path, with no extra visibility state,
service or timer. The server ACK supplies the actual send time for offline
queued messages. The range includes all observed eligible keys at its boundary
second, with each author and from-me flag. Schema 20 retains a revoked bubble's
separate read key and freezes the bounded key list in the existing outgoing read
queue. Retrying an older read cannot acknowledge a deletion received afterward.
A changed key list queues a new update even when the local badge is already
clear. If an older queued identity is unavailable, its original bounded timestamp
fallback remains. Repeated ACKs/revisits deduplicate by position and key list,
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

## Revoked identities and same-second ranges — 2026-10-30

The earlier deletion fix cleared Heya's local badge but retained the original
remote key. A synthetic original-read → confirmed-sync → later-revoke → viewed
tombstone sequence reproduced a missing outgoing update. A single-key range
also omitted other observed messages sharing its final timestamp.

Inspection of unauthenticated public WhatsApp Web bundles established the read
identity distinction: `WAWebRevoke` and `WAWebDBProcessRevokeMsgs` replace a
revoked bubble's ID while retaining its original position and author.
`WAWebDBMessageRange` gathers boundary-second keys, and
`WAWebMessageRangeUtils` compares their coverage. Heya implements those rules
independently through the SDK's existing public app-state API; no SDK patch,
extra worker or polling timer was added.

Source artifacts inspected, without account access:

- [Revoke modules](https://static.whatsapp.net/rsrc.php/v4/y3/r/Tp-PhBry3trEdvTxznVWky.js), SHA-256 `dd3efbdc1ae0b462594b5cac10ea7caff83fe017a78586dfafda8772d2153b2f`.
- [Read/range modules](https://static.whatsapp.net/rsrc.php/v4izcG4/yt/l/en_US-j/Q0Jy7eM4erM.js), SHA-256 `9471b7e41356da79d3e5b593ba661ac6811a25dbbaccca1bd81d3eea3e246afd`.

The archive upgrade recovers older deletion IDs only for unambiguous
one-event/one-target pairs. Previous versions discarded accepted protocol
bodies, so ambiguous older pairs cannot be reconstructed safely. The migration
does not publish reads automatically; viewing the conversation creates the new
range. New ingestion retains exact identities directly. Work is restricted to
one indexed boundary second, with the protocol's 1,000-key limit; an oversized
second clears the exact local observation but does not publish a partial remote
range. A later timestamp permits synchronization again.

Synthetic tests cover multiple same-second deletions, already-read originals,
out-of-order edits/deletes, early read-self receipts, newer unobserved arrivals,
immutable offline retries, stale completions, archive upgrades and reopen.
These establish local serialization and state ordering, not delivery or conflict
resolution in another live client.

Linux offscreen validation passed 355 default-feature tests, 336 media-disabled
tests and the encrypted protocol-storage integration test in each configuration.
Strict all-target Clippy passed in both configurations. A paired optimized
10,000-message archive/UI check retained comparable application timings:
incoming UI-application p95 was 7.18 → 6.94 ms, and navigation SQLCipher
projection p95 was 11.50 → 11.94 ms. Both runs used the private X server and
Skia CPU renderer; these are application timings, not GPU/compositor or live
account measurements. No account messages were sent by automated checks.

## Deleted read-boundary regression — 2026-10-20

Before changing application code, three SQLCipher regressions failed against the
existing implementation: viewing a deleted final message left unread at 1;
viewing an expiry placeholder also left it at 1; and reading through a deleted
row with a newer same-second arrival left 3 unread instead of 1. The timeline
reported the visible placeholder, but the archive rejected it as a read boundary.

The correction removes payload deletion/expiry exclusions from the two visible
read-position queries. Hidden rows, chat identity, exact `(stamp,id)` boundaries,
and the confirmed-send requirement for outgoing positions stay enforced.
Sender-facing receipt eligibility is unchanged. Those original tests covered
direct/group chats and the original timestamp; they did not cover the separate
revoke read key or all boundary-second IDs, corrected above. They also exercised
and the headless active-window read gate for an unavailable deleted bubble.
The optimized default suite passed 228 tests; the audio-disabled suite passed
210. Both configurations retained three intentional skips and passed strict
all-target Clippy with warnings denied. The UI check uses Slint's headless
backend and never opens a desktop window. Automated checks use synthetic
archives and do not establish live propagation
to other WhatsApp clients.

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
