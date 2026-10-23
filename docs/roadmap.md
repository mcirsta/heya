# Heya implementation roadmap

Preparation is complete; M0 implementation and the first M2 text/archive slice
are in progress. All milestone
acceptance gates below remain open until their evidence is recorded. Linux and
macOS are active targets; Windows follows later. Each milestone ends with a working
demonstration and recorded evidence, rather than an assertion based on library
capabilities.

ZapFast supplies the visual reference and can inform interaction ideas. These
milestones define Heya's own implementation and do not require ZapFast feature
parity or reuse of its application code.
Omarchy-specific integration is deliberately outside Heya's scope.
Keep whatsapp-rust unchanged where its public interfaces cover Heya's needs.

## Current delivery priorities

This is the order for new work across the open milestones, prioritized on
19 October 2026. The table sets delivery goals; current progress and outstanding
validation are recorded below. Complete
one bounded feature, its failure paths and validation before starting the next.

| Order | Work | Completion criteria |
| --- | --- | --- |
| Release gate | Reliability of existing paths | Keep CI green. Deliberately verify voice capture/playback/delivery, ordinary direct/group edits and deletion, session resume and interrupted history/network recovery on Linux and macOS. Record remaining interoperability/device gaps with each release. Advanced variants remain separate work. |
| 1 | Sender-facing read and voice played receipts, with receipt privacy | Specify and honor protocol/account privacy behavior before enabling receipts. Preserve the distinction from private linked-device unread synchronization. Hidden messages and audio downloads must not trigger receipts. Validate offline/restart behavior and direct/group cases. |
| 2 | Local message search and quote navigation | Search the encrypted archive off the UI thread, including older cached messages. Edits update results; deleted/expired content stays excluded. Selecting a result or quote locates and highlights the original message, preserving a way back; unavailable originals get a clear state. |
| 3 | Attachment paste/drop and copying images | Reuse the existing file/photo review and upload path. Preserve the intended recipient, caption and text draft; never send automatically. Cover clipboard images/files, duplicate events, cancellation and native platform integration. |
| 4 | Video playback, then GIFs and animated stickers | First finish common video playback with audio, seek, pause, error recovery and teardown. Use native Apple media frameworks on macOS and existing GStreamer on Linux. Add other animation formats as separate validated increments. |
| 5 | Desktop notifications and existing-window activation | A second launch activates the existing window. Notifications honor preview/privacy settings, open the intended conversation and handle unavailable services or denied permissions. Use native platform services. |
| 6 | Richer message text | Add bold/italic/code formatting, inline links and mentions while preserving original text, copying, selection, Unicode and stable timeline geometry. Validate each text behavior before combining them. |
| 7 | Chat management | Implement manual read/unread, pin/archive/mute, then favorites and the remaining filters. Persist real state and synchronize supported actions; do not present placeholder filters as working. |
| 8 | Additional cards and advanced interaction | Polls, contact and location cards, cross-message drag selection and remaining advanced message-control variants follow the everyday messaging work above. Scope and validate each separately. |

The first chat-management increment synchronizes mute/archive/lock state and
adds mute durations, unmute, archive/unarchive and Lock controls. Locked chats
remain hidden until unlocked on the phone. Pinning, favorites, manual read/unread
controls and local unlock authentication remain separate work. See
[chat management](chat-management.md) for validation and interoperability limits.

The first slice now implements **sender-facing read and voice played receipts**
through the pinned SDK's public APIs, with a local privacy switch and account
privacy fetch before transmission. A separate encrypted queue records visible
reads and confirmed voice playback without backfilling historical read messages.
See [receipt behavior and validation limits](read-sync.md). Local macOS synthetic
tests cover queueing, privacy, cancellation and playback progress; Linux CI and
later deliberate Linux/device/account checks remain required. This is not a
completed release gate or a claim of official-client parity. Broader account
privacy editing remains on the phone for this slice.

The second slice adds conversation-local, Unicode-aware literal search over the
encrypted archive, bounded to the newest 100 matches, plus quote/result navigation
through a 100-message window and a return-position anchor. Search excludes hidden,
deleted and expired messages and refreshes after archive changes. Both platforms
share the worker and UI code; local macOS synthetic tests and Linux CI cover the
implementation. Deliberate live-account checks and later Linux desktop validation
remain open.
Clipboard/drop handling shares the existing attachment flow rather than creating
a second sending pipeline.

The first received-video slice adds explicit inline/expanded playback, pause,
seek and mute with one finite decoder process and encrypted original caching.
Linux uses GStreamer; macOS has a native AVFoundation implementation awaiting
native validation. GIF looping, animated stickers, video notes and video sending
remain separate increments. See [video playback](video-playback.md) for bounds,
teardown and reproducible offscreen checks.

The photo viewer now uses explicit album associations for navigation, counters and
a bounded thumbnail strip, independently of timeline paging. See
[photo albums](photo-albums.md) for storage, decoding bounds and validation limits.

Performance regression checks accompany each increment. Existing before/after
measurements establish changes within Heya, not speed parity with ZapFast;
offscreen application/render timings do not establish compositor, device or
end-to-end account performance. These priorities do not close M0 or M2 gates.

## M0 Prove the toolkit integrations

The initial Cargo application and offline Slint demo now exist. Current code
includes light/dark/system themes and eight named palettes, a searchable settings
page with persisted presentation preferences and explicit unfinished items,
chat search and unread filtering, draft restoration,
local sends, per-message text selection, loaded-transcript copying and a paged
10,000-message fixture. These are a starting point, not completion of the text,
clipboard, media or performance acceptance gates below. Tests also exercise the
protocol store's connection-init hook with the selected SQLCipher build.

Create the Cargo application scaffold with the proposed dependency pins and
commit its resolved lockfile. Keep this first prototype offline and use
synthetic messages/local media. Use the actual combined dependency graph to
catch native SQLite linking and feature conflicts early.

| Check | Done when |
| --- | --- |
| Build | Pinned stable Rust builds Slint and whatsapp-rust together; no nightly-only defaults leak in; one SQLCipher provider is selected |
| Text | Multiline IME input, emoji, RTL and formatted links work; word selection and copying across message components work while scrolling |
| Timeline | 10,000 variable-height synthetic messages scroll; prepending history and late image dimensions preserve the viewed message |
| Clipboard | Text, screenshots and copied files paste once into the correct destination on Wayland and X11; focus and file-thumbnail precedence pass |
| Media | A local H.264/AAC MP4 plays with sound, seeking and inline/expanded switching; a failed decode is recoverable; closing releases playback |
| Storage prerequisites | SQLCipher availability and wrong-key failure are demonstrated with the protocol connection-init hook |

An opt-in archive benchmark now drives SQLCipher, the real messaging worker,
coalesced UI application and render completion with a disposable synthetic
500-chat/10,000-message archive. It separately reports queue, projection,
publication and end-to-end application latencies, including actual history
paging and incoming/unrelated bursts. This closes the prebuilt-snapshot coverage
gap; it does not establish network, input-device, GPU or compositor latency.
The archive retains one immutable loaded timeline and transcript, invalidated
by relevant database writes. Unrelated message updates and composer drafts reuse
that projection while sidebar, reply and permission state refresh independently.
Strict appends decode only new rows and reuse unchanged transcript text; edits,
deletions, out-of-order history and shared metadata changes rebuild the projection.
The benchmark reports full/append projection, formatting and cache reuse explicitly.

Record renderer/backend versions and identify any required Slint fixes. If a
check exposes a missing primitive, implement a narrowly scoped adapter or a
tracked upstream patch before expanding the application. Do not substitute a
webview or switch UI frameworks without a new product decision.

## M1 Build the offline UI

Implement the component map in [the UI specification](product-and-ui.md) with
mock service data: chat list, filters, conversation, composer, menus, pairing,
settings, viewers and error states. Extract theme tokens into Slint and keep
all user-facing text ready for translation.

Done when dark/light screenshots at the target sizes reproduce the reference's
structure and density, keyboard focus survives chat/menu/viewer transitions,
and text, reply/edit and staged attachments retain their destination and draft
state. Provide a `--demo` mode that neither reads real account files nor opens
a protocol connection.

## M2 Connect messaging and durable storage

Saved call events reuse the encrypted timeline and sidebar, with voice,
video, missed-call, direction and available outcome/duration metadata. Public
SDK app-state, captured history and call-log envelopes feed the same projection;
calls never become message unread/receipt/outbox keys. Initial state recovery
requests call logs previously ignored. One-to-one audio calling now has a separate
controller and finite duplex audio helper; see [audio calling](audio-calling.md).
Direct two-way video calling extends that controller with a finite camera/codec
helper, bounded frame presentation and camera/mute/hangup controls; see
[video calling](video-calling.md). Hardware and account interoperability still need
manual checks on each platform; synthetic tests do not establish those.
See [call events](call-events.md); the owner confirmed a real event in both the
sidebar and conversation. Actual call interoperability is still unverified.

Local storage now offers an initial encrypted/unencrypted choice and a Files
settings switch. Mode conversion uses normal shutdown/restart, verified exports
and an atomic manifest, preserving the session and archive together. Plaintext
startup does not access the key store. Synthetic conversion, interruption and UI
checks are described in [local storage](local-storage.md); OS/account validation
on other machines remains open.

The first slice now implements Connect/Cancel/Disconnect, QR rotation/expiry,
encrypted single-session storage with a desktop keyring key, configurable saved
session resume, offline archive opening, SDK reconnection states and bounded
shutdown. Confirmed unlink/relink preserves the archive and its account binding;
an unconfirmed remote unlink is reported. The upstream pin is unchanged.
Automatic connection is enabled by default. Disabling it waits for Connect
without opening the keyring or archive; an explicit `--offline` launch still
opens the encrypted archive and needs its key. A failed or cancelled keyring
unlock remains an explicit connection failure, with manual retry.
Synthetic tests cover storage, state mapping, reset/archive preservation and UI controls;
the owner has reached the connected chat list after pairing, and a later launch
resumed the existing session to Connected. The owner confirmed live receiving.
The owner later confirmed manual attachment delivery to another client on
17 October 2026. Network interruption and broader outgoing cases remain
unverified. This work started alongside the still-open M0 checks.

Window-close cleanup now releases the published audio client before waiting for
SDK storage ownership. The previous ordering retained the session lock until a
cleanup timeout. Regression checks cover repeated encrypted reopen, pending
audio cancellation, and native close/process exit on the private X server.
Full single-instance activation (raising an already-open window on relaunch)
remains pending.

The text/archive slice adds a separate SQLCipher database and key, direct/group
text ingestion, SDK inbound/history hooks, bounded staged history import, PN/LID
merges, stable timeline ordering, durable drafts and a persistent outbox with
explicit uncertain delivery. It also adds cached chat views, new chats by phone
number and receipt footers. Synthetic replay, rollback, encryption, send/draft
handoff, receipt and shutdown tests cover these paths. Phonebook events now feed
a separate encrypted contact table, with snapshot recovery for older sessions
and unclean exits. The protocol pin remains unchanged.

Right-click message controls now include ZapFast-style quick reactions,
reply/forward, selection, text copying, eligible own-text editing, sender
revocation, deletion for me, message info and wire-ID copying. Selection is
bounded to 100 messages and uses the existing virtualized timeline. The
protocol adapter uses public SDK edit and chat-action APIs, with sender revoke
envelopes sent through the durable outbox. Edits/revokes change local content on
acknowledgement; linked-device deletion is queued independently from local
visibility. Synthetic archive/outbox/UI coverage does not establish live
server acceptance. Community-announcement edits requiring a message secret,
administrator revocation and complete historical per-recipient receipts remain
outside this slice. Received typed caption edits now preserve media cards and
cache entries when transfer fields are unchanged, with current envelopes used
for quoting/forwarding. Canonical author identities separate group sender runs
even when display names collide. Synthetic regression coverage includes out-of-order
edits, caption removal, archive upgrades/reopen, aliases and narrow UI layouts.
See [message controls](message-controls.md).

Saved history now expands automatically in 100-message pages when the reader
reaches the top, and fills short viewports without a load button. The existing
message-identity anchor preserves the viewed content while prepending. One
request at a time, navigation revisions and matching completion tokens bound
work during scroll/update bursts; query failure leaves the previous page size
intact and retries on a fresh approach or chat reopen. Empty chats show “No
messages yet” and skip automatic paging; explicit phone fetch remains available
without marking unknown history exhausted. This pages the encrypted
archive first, then can request up to 50 additional messages through the SDK's
public phone-history API. Captured chunks and request IDs are correlated only
after import completes; duplicates, aliases, terminal transfer flags, timeout,
backoff and cancellation are covered synthetically. There is also an explicit
conversation-menu action. The owner confirmed live older messages loading from
the phone on 17 October 2026. Completeness, phone-offline behavior and network
recovery remain deliberate account-validation gates.

On-demand contact/group/account avatar thumbnails now use a separate bounded
worker and an encrypted schema-3 cache, including offline display, picture
notifications, privacy/no-picture fallbacks and stale-download protection.
Visible incoming group sender runs now share that worker/cache for participant
pictures, without creating private chats. Group headings distinguish profile
names (`~`) from saved contact names. Synthetic checks cover aliases, removals,
bounded demand and stable layout; live participant-picture verification remains
open. Full-size profile-picture viewing remains pending.

The open direct conversation now subscribes to online/last-seen presence through
the unchanged SDK. A single transient slot feeds the header without rebuilding
message models; navigation, reconnect and shutdown clear stale status. Groups,
self-chat and unknown/private presence omit it. Synthetic transport and header
layout tests cover the path; live-account availability remains unverified.

Incoming read-state synchronization now handles own-device read/played receipts,
app-state read ranges/manual unread marks, history totals, reconnect recovery and
PN/LID merging. A schema-9 durable queue now sends private local read positions
back through the SDK's app-state API. Active-window/timeline visibility gates
local reads; serial background retries preserve the displayed boundary. Synthetic
archive, service, transport and toolkit tests cover this path. Live propagation
to the phone and ZapFast still needs owner verification. Sender-facing read and
played receipts now use their own durable queue and local privacy switch, with
the account setting checked before sending. Live interoperability remains open.

Business templates expose supplied text, clickable URL actions and copy-code
buttons. Received link previews use sender-supplied metadata and on-demand
embedded thumbnails. Schema 5 stores this metadata in the encrypted archive;
startup recovers older rows from saved envelopes. Interactive business call/reply,
payment/flow actions and carousel cards remain pending.

Incoming pin/unpin events now map to typed, centered system notices in both live
and history ingestion. A versioned content repair upgrades saved envelopes;
notices cannot be used as reply/reaction/forward sources. Pin actions and the
pinned-message header are separate, unfinished features.

Message hover controls now expose reactions, quoted text replies and explicit
single-recipient forwarding. A schema-8 migration stores reply drafts and a
reaction projection; rich outgoing envelopes use the existing durable outbox.
Forwarded photos/audio retain SDK media references, while business cards become
plain readable text. Individual incoming reaction events and quotes display;
aggregate reaction snapshots in history remain pending. These paths have
synthetic validation only; live account interoperability is still open.

Remaining: full local account/data removal, complete edit/delete variants,
typing, broader account privacy controls, full-text search, live chat
metadata/settings updates, recovery UI and deliberate account tests. Received media support is described under M3. Basic disappearing-message expiry does not establish
complete lifecycle parity. M2 is not complete.

Implement the protocol adapter, QR pairing, session resume, connection states,
encrypted archive, migrations, inbound/history durability and persistent outbox.
Wire direct/group text, history pagination, quotes, reactions, edits/deletes,
typing and receipts. Add local search over the archive.

Done when deliberate testing with the owner's chosen account covers pairing,
restart without re-pairing, send/receive, phone edits/deletes, history overlap,
network interruption and resume. Automated replay tests cover duplicate events,
equal timestamps, PN/LID identities, failed transactions and interrupted sends.
Crash/restart must not duplicate an already reconciled send or resurrect a
deleted message. Report ambiguous delivery rather than inventing a success state.

## M3 Complete everyday media and clipboard use

In progress: received JPEG/PNG photos use authenticated SDK streaming downloads,
encrypted original/preview storage, native inline pictures and a conditional
larger viewer. Download/decode demand follows nearby rows, with size limits,
manual retry, an automatic-download preference and offline cached viewing.
Synthetic tests cover storage/lifecycle/layout; live received-photo compatibility
still requires the owner's check. Received audio now has a single GStreamer
player, authenticated on-demand downloads, encrypted cache, waveform seeking,
pause/resume and speed changes. Automated playback uses synthetic Opus/WAV and a
silent sink. Playback helpers are created only when audio is ready and exit on
stop/completion/failure; tests cover idle/download paths, rapid replacement and
teardown, retry and replay. Real audio-device/account validation remains open.
Voice recording/sending now has on-demand GStreamer Ogg/Opus capture, pause/resume,
review/playback, explicit Send, streamed AudioMessage/PTT upload and an encrypted
local audio cache committed with the outgoing envelope. Audio settings persist
microphone/speaker choices and recording/playback levels, with on-demand device
discovery and explicit missing-input rejection. Downloads have a persisted initial
Save As folder, defaulting to the portable system Downloads location plus Heya.
Real microphone and
recipient playback checks remain open. Video playback, disk-cache eviction
and expired-CDN recovery remain pending. Received document cards now display
sender-supplied thumbnails, filename, type, size and optional page count. Existing
encrypted envelopes upgrade locally; decoding and thumbnail residency are bounded
off the event loop. Nearby cards now request the separate authenticated HD
thumbnail, with a 1 MiB transfer limit, an encrypted strip cache and
768×320 maximum retained pixels. The document-thumbnail media type requires
a small pinned SDK patch tracked in `patches/README.md`. PDFs without a sufficiently
sharp supplied image now use authenticated document download (16 MiB maximum)
and isolated Poppler first-page rendering. Only the resulting strip is cached;
schema 11 lets old low-resolution entries upgrade while preserving their pixels.
Explicit attachment Download now uses the Linux native Save As portal, streams
authenticated originals to a private staging file, and publishes the selected
file only after verification. One transfer runs at a time with a 2 GiB limit,
Cancel/Retry and independent progress. Cached photo/audio exports work offline.
Outgoing file/photo selection, review, captions, streaming upload and durable
queueing are now implemented through the SDK public API. Up to ten files share a
2 GiB selection limit; JPEG/PNG photo originals are bounded to 20 MiB/32 MP.
Retries retain unsent items without duplicating queued ones, and protocol source
ownership is released on shutdown. Upload-specific workers are created on demand
and joined after completion; unnamed encrypted staging storage is released on
close/process exit. Unsubmitted selections are session-only.
Clipboard photo/file paste and X11/macOS file-drop handling now reuse encrypted
attachment review and explicit sending. Copy image uses cached original pixels.
Native Wayland drops use the pinned Slint development/winit 0.31 backend. Native
macOS clipboard/drop acceptance remains unverified. The development backend
disables the native accessibility bridge; restore it when Slint supports it. See [sending attachments](attachments.md).
Opening files inside Heya and video playback remain separate work. See [voice messages](voice-messages.md)
for recording behavior and validation.
See [downloads](downloads.md) for behavior and validation; desktop transfer RE
and SDK API notes are recorded in `docs/desktop-media-audit.md`.

Implement upload/download, thumbnails, image preview, audio/video playback,
copy image, save as, file picker, drop/paste, per-chat attachment drafts,
cancellation, cache policy and expired-media recovery through the SDK.

Done when the [clipboard and media acceptance cases](product-and-ui.md) pass
with real desktop applications. Include missing files, invalid images,
unsupported codecs, low disk space, cancel/retry and changing chats during
operations. Media must not block typing or history scrolling. Large downloads
have an explicit policy and accurate progress/failure states.

## M4 Ship a Linux preview

The selected Heya logo is now an embedded SVG, with a matching Linux application
ID, desktop-entry template and per-user development installer. The installer
also renders PNG icon fallbacks. This does not establish clean-install,
compositor or packaging compatibility; M4 remains open.

Add notifications with privacy controls, desktop entry/icon, About/credits,
accessible labels, settings persistence and a clean account-removal flow.
Choose the first package after testing codec/keyring/portal availability on a
clean target system; evaluate Flatpak for a consistent runtime and an ordinary
package for local development. Use Heya's own application identity.

Done when a clean Linux install passes Wayland and X11 smoke tests, starts and
exits cleanly, and handles unavailable keyring/audio/codec services. Record a
dependency inventory, retained notices and actual installed codec set. No updater
is required for this preview. Publication metadata can be settled at release time.

## M5 Expand feature and platform coverage

The initial macOS build and native service adapters are now described in
[macOS development](macos.md). A successful build and synthetic tests do not
close the desktop and live-account acceptance gates below. Native macOS audio uses Core Audio output queues and Core Audio/AudioToolbox, with synthetic
codec and lifecycle coverage; hardware and recipient checks remain open. Native
video playback remains pending.

Add Windows and macOS builds, native clipboard/menu/notification/keyring
behavior and installers. Then prioritize polls/stickers/GIFs,
advanced organization and additional accounts. Calls and other advanced service
features remain separate projects within the roadmap, after core messaging is
stable.

Each platform needs actual execution tests, especially paste/file-drop ownership,
Unicode/IME, scale changes, suspend/resume, background notifications, codecs and
signing/packaging. Cross-compilation alone is insufficient.

## Validation strategy

| Layer | Verification |
| --- | --- |
| Domain and storage | State transition, merge/deduplication, pagination, draft, outbox and migration tests with synthetic data |
| Adapter | Recorded synthetic event shapes at the pin; contract tests for SDK mapping and failure categories |
| UI | Offline fixtures, selected visual comparisons, keyboard/accessibility checks and interactive clipboard/media tests |
| Live protocol | Opt-in, deliberate account testing; no automated unsolicited replies or default test sends |
| Release | Clean install, native dependency inventory, package codec tests and platform lifecycle checks |

For application changes use `cargo fmt --check`, `cargo clippy --locked` for the
relevant targets/features, and `cargo test --locked`. Test media-enabled and
media-disabled builds; avoid `--all-features` if platform/media features become
mutually exclusive. Add CI once there is an actual workspace to validate.

## Performance targets

These are initial targets to measure, not observed Heya results. Use a documented
reference machine, release build, 10,000-message fixture and representative media.

- Keep 95th-percentile frame time within 16.7 ms while scrolling on a 60 Hz display.
- Present the cached chat view within one second of a warm launch.
- Keep memory bounded as history and media are paged; record idle/peak RSS and
  verify it stabilizes after repeated chat/video open-close cycles.
- Avoid continuous redraw while idle. Measure CPU and wakeups, including when
  hidden or minimized, before choosing a numerical idle budget.

Measure text/clipboard correctness before optimizing it away. ZapFast's published
performance numbers are context, not transferable results for a Slint rewrite.
