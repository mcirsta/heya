# Received contacts and round video messages

**+ → Contact** sends contacts from the phonebook synced by WhatsApp. Search
names or numbers, select one or several people, then choose Next to review the
recipient and exact names/numbers before Send. A single choice sends a contact
card; multiple choices send one contact bundle. Heya shares the selected name
and WhatsApp phone number, without adding unrelated phonebook fields.

Search uses the archive worker with Unicode matching and at most 100 results;
refine the search when more are available. Selection survives searches. Known
phone numbers are required; unresolved LID-only entries and locked contacts are
excluded. Avatar demand follows the visible picker rows through the existing
bounded cache. The chooser adds no service, upload or idle thread.

Explicit Send saves an ordinary contact envelope to the existing durable outbox.
The save acknowledgement closes the dialog before the full conversation rebuild.
Repeated clicks are gated, failures preserve selection, and late results cannot
reopen a closed or navigated-away dialog. Existing reply validation, ephemeral
expiration, disconnect/shutdown and outbox delivery behavior apply. The composer
draft is preserved. Quotes share one protocol helper with file/photo uploads.
The SDK public message/context APIs support both contact types unchanged.

Received contact messages and contact bundles display an inline contact card.
It opens a scrollable details dialog with selectable phone numbers, email,
organization, address, website, birthday and notes. Contact bundles open a list
first. Copy details uses the existing clipboard path. Message opens Heya's
normal new-chat flow for sender-declared WhatsApp numbers, preserving the current
draft and reporting offline/busy failures through the existing UI. Displaying a
card does not add it to the phonebook or launch supplied websites.

Save contact file and the message's Download action export the original vCard
through the existing native Save As service, including while offline. Export
uses the normal identity, cancellation, integrity and late-deletion guards.
No network worker is needed for the vCard bytes already in the envelope.
Inline profile pictures use the bounded avatar service and fall back to initials.

vCard display parsing supports folded lines, escaped separators/newlines and
UTF-8/Latin-1 quoted-printable fields. Parsing is bounded to 257 cards, 5 MB of
card data, 100 logical fields per card and limited display-field lengths. Original
envelopes are retained for forwarding and file export. Unknown fields and external
PHOTO URLs are not fetched or interpreted as executable actions.

Round video messages (`ptvMessage`) share the existing authenticated video
transport, poster selection, encrypted cache and finite player. Their inline
view crops to a circle with play/pause, seek, mute and expansion controls. The
control area is reserved so playback does not shift the timeline. Linux uses
GStreamer and macOS uses AVFoundation; this feature adds no media service.
Played receipts require actual playback progress, honor the existing privacy
setting and reuse the receipt queue. Ordinary videos do not gain played receipts.

The SDK already exposes both received types. Its privacy helper omitted inline
PTV view-once flags; the isolated `ptv-view-once.patch` fixes that omission with
a library regression test. View-once media still produce the phone-only message
and are not cached, downloaded or played. The patch is pinned and reproducible
through the existing vendoring script, with status in `patches/README.md`.

Startup text decoder revision 7 repairs eligible older unsupported contacts and
PTV messages in place. Identity, timestamps, read state and drafts are preserved;
deleted, expired and mutated content is excluded. The same extraction rules apply
to live messages, history and repair.

Public WhatsApp Web contact parsers, vCard/card renderers and PTV parser/renderer
were inspected on 5 November 2026; bundle URLs and hashes are recorded in
`prep/upstreams.json`. Heya's parsing, storage, controls and lifecycle code are
independent. Synthetic data can be viewed with:

```sh
HEYA_DEMO_SHARED_CONTACTS=1 scripts/ui-offscreen.sh --framebuffer /tmp/heya-contact-video.png --demo-chat 7 --size 1200x1000
```

Synthetic tests exercise decoding, wrappers, old-message repair, offline export,
cache/receipt guards, modal actions and geometry in light/dark and narrow layouts.
The native video check uses generated MP4 files and a silent audio sink. These
checks do not establish real contact registration, recipient receipts or macOS
hardware interoperability; automated tests send no account messages.

Linux validation on 5 November 2026: 395 default-feature tests passed (13
deliberately ignored), and 376 passed without media (12 ignored). The UI callback
test passed again after the final geometry corrections. Strict release Clippy,
formatting and patch reproduction passed. Private Skia/OpenGL framebuffer checks
covered cards, bundle/details dialogs, dark/narrow layouts and playing round
video. The native check passed progress/privacy receipts, four generated MP4
variants, pause/seek/mute/EOS and explicit-stop/closed-pipe process reaping.

The optimized mixed-message-card benchmark exercised 500 chats and up to 10,000
loaded rows. Render CPU p95 was 11.82 ms during 10,000-row scrolling versus
13.16 ms in the previously installed build; snapshot application p95 stayed
0.36 ms. Warm resizing p95 was 13.59 ms versus 13.37 ms. Both runs retained a
first-resize render spike (504 ms updated, 511 ms baseline), so this check does
not establish stall-free resizing. These are private Xvfb/Mesa measurements,
without desktop compositor presentation, and the bulk benchmark uses existing
mixed cards; the new contact/PTV layouts are covered by the separate UI checks.

Contact sending validation: 399 tests passed with media and 380 without media;
strict release Clippy and formatting passed. Tests cover single/bundle SDK
round-trips, escaped/Unicode names, quotes, 257-card limits, locked/LID-only
exclusion, bounded Unicode phonebook search, durable queueing and draft
preservation. UI checks exercise queued search edits, selection across searches,
duplicate clicks, failed saves, rejected commands, navigation and late replies.
Private framebuffer checks covered selection/review in light/dark and narrow
windows, a demo-only outgoing bundle and stopping hidden inline video.

The same optimized mixed-card benchmark after sending support measured render
CPU p95 of 14.02 ms for 10,000-row scrolling (installed baseline 13.49 ms),
12.93 ms for warm resizing (13.42 ms baseline), and snapshot application p95
0.40 ms (0.39 ms baseline). Both retained the existing roughly 514 ms first-resize
spike. This paired smoke check does not establish real compositor performance,
macOS behavior or live recipient compatibility. Sending to an actual account
still requires a deliberate manual test; automated sends use only the demo.
