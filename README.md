# Heya

![Heya logo](ui/assets/heya.svg)

Heya is a native WhatsApp desktop client in development, written in Rust, using
[whatsapp-rust](https://github.com/oxidezap/whatsapp-rust) for the WhatsApp Web
protocol and [Slint](https://slint.dev) for the interface. Its starting design
is [ZapFast](https://github.com/crmne/zapfast), used as a visual reference and a source of interaction ideas.
Heya's behavior, storage, media and application architecture are implemented
independently, with particular attention to video and desktop clipboard handling.
Use whatsapp-rust unchanged through its public API wherever possible.

Generated builds and validation outputs are disposable. See
[workspace cleanup and reproduction](docs/workspace-cleanup.md) for the safe
cleanup command and how to recreate them; application binaries belong in releases.

**Status: first live text-messaging and encrypted-archive implementation.**
Linux is the reference implementation, and the native macOS port is now active.
Windows is the next planned target. Heya implements QR
pairing, encrypted session resume, incoming direct/group text, history import,
saved drafts, paged conversations, outgoing file/photo attachments, voice recording/sending and a durable outbox. Automated tests
are offline. The owner has reached the connected chat list, and reopening the
saved session has reached Connected. The owner has confirmed live receiving and,
after the upload-cancellation fix, attachment delivery to another WhatsApp client.
Other attachment types, batch/large transfers and network recovery still need
deliberate live validation. M0 and M2 are still open.

The layout follows the pinned ZapFast UI declarations. See the
[source audit](docs/ui-source-audit.md) for dimensions, typography, themes and
remaining differences. Earlier prototype screenshots are not the current UI.

## Run Heya

For Apple Silicon Macs running macOS 15 or newer, download the
[testing release](https://github.com/mcirsta/heya/releases/tag/v0.1.0-alpha.3).
See [testing and first-launch instructions](docs/releases/v0.1.0-alpha.3.md).

On macOS, see [the native macOS build guide](docs/macos.md). The macOS build
uses Metal, AppKit file dialogs, Keychain and Quartz PDF previews. Native audio
uses Core Audio output queues for playback and Core Audio/AudioToolbox for microphone capture
and Opus encoding. Launch the `.app` bundle to grant microphone permission;
GStreamer is only used on Linux.

Received-video playback uses GStreamer on Linux and a native AVFoundation path
on macOS, with explicit play/pause, seek, mute and inline/expanded views. See
[video playback and validation limits](docs/video-playback.md); native macOS
video validation remains open.

macOS startup diagnostics are saved automatically under `~/Library/Logs/Heya`.
If the process appears in the Dock without a window, retain the newest logs and
try `--software-rendering --demo` after closing it. See
[GUI startup diagnostics](docs/macos.md#gui-startup-diagnostics) for checkpoints,
bounded retries and the native packaged-GUI check.

macOS adaptations are scoped to that platform. Linux keeps its existing media,
desktop services, renderers and layout; CI builds and tests Linux with and
without GStreamer support.

```sh
cargo run --release --locked
```

On Linux, the default `media-gstreamer` feature links the installed GStreamer runtime and
app library. `--no-default-features` builds the UI/domain without playback, recording or call audio.
Use the optimized build for everyday desktop testing. `cargo run --locked`
is the slower development build.
The toolkit is pinned to a Slint 1.19.0 development snapshot using
winit 0.31.0-beta.3 for native Wayland file drops. Versions and source revisions
are recorded in `prep/upstreams.json` and `Cargo.lock`; no floating branch is
used. **Known limitation:** this Slint branch disables the native accessibility
bridge, so screen readers do not receive Heya's accessibility tree on Linux or
macOS. Keyboard navigation remains available. Restore the bridge when Slint
supports it with winit 0.31; Heya does not carry a separate AccessKit port.

The Linux default prefers Slint's Skia/Vulkan renderer, selected after synthetic
scroll/resize comparisons on the development machine. OpenGL remains enabled
for automatic fallback if Vulkan initialization fails. Launch normally without
`SLINT_BACKEND` to use this selection; `SLINT_BACKEND=winit-skia-opengl` explicitly
selects OpenGL. This startup fallback does not guarantee recovery from a driver
failure during use.

The window and connection screen use Heya's SVG logo. After building, run
`python3 scripts/install-desktop.py` to install the current binary, application-menu
launcher and taskbar icons for your user. The desktop integration uses the SVG
where supported and generated PNG fallbacks otherwise. See
[branding and desktop integration](docs/branding.md) for paths and update steps.

Saved sessions resume automatically on a normal launch. On first use, click
**Connect** to display a QR code and scan it from WhatsApp's **Linked devices →
Link a device** screen. To open the real encrypted archive without connecting,
run `cargo run --release --locked -- --offline`; **Connect** remains available.
**Disconnect** closes the connection and retains the session. The SDK handles
temporary network interruptions while the connection is active, and a banner
shows the connection state without hiding cached conversations.

**Settings → Account → Unlink** asks for confirmation before requesting remote
unlinking. It preserves saved chats. A revoked or successfully unlinked session
offers **Link again**, also with confirmation; this replaces protocol credentials
while preserving the encrypted archive and its account binding. Scan with the
same WhatsApp account. An unconfirmed remote unlink is reported explicitly;
reconnect or remove Heya from the phone's linked-device list.

After connecting, choose a received chat or enter a phone number with its country
code using the **New chat** header button. Enter sends by default; Shift+Enter
inserts a newline. **Ctrl+Enter** or the send button queues text for that conversation. The composer clears only after the message
is saved to the encrypted outbox. Drafts persist independently for each chat.
The save confirmation reaches the composer before the conversation view is rebuilt.
While saving, the send button is disabled and the shortcut hint shows the status;
holding Enter does not resubmit text.
With an empty composer, the microphone starts a voice message. Pause/resume or
Stop to review, then play, discard or Send. Recording is bounded to 30 minutes
and 20 MiB; review data is discarded on close. See [voice messages](docs/voice-messages.md)
for behavior and validation. Settings → Audio selects input/output devices and
recording/playback volumes. If no microphone is available, clicking the microphone
opens a “Microphone not found” dialog with setup guidance; no recording starts.
The composer smiley opens the shared emoji picker. Search names/shortcodes,
use category buttons or arrow keys, and choose an emoji to insert at the cursor
or replace the selected text. The picker stays open for repeated insertion;
Escape closes it and returns focus to the draft. Recent/frequent lists currently
last for this application session.
Chat text and reaction badges use color emoji. Messages containing one to three
emoji use the reference’s larger size; mixed text stays at normal size. See
[color emoji rendering](docs/color-emoji.md) for the isolated text-shaper fix.
The chat list shows a draft after you leave that chat; the open chat keeps its
last-message preview while the saved draft remains in the composer.
Scrolling near the top automatically expands saved history in pages of 100,
keeping the viewed message in place. Short pages fill the available space
automatically. Failed page loads retry when you approach the top again or reopen
the chat. Once cached history is exhausted, a connected Heya requests up to 50
older messages from the primary phone. The conversation menu also offers **Fetch
older messages from phone**. Keep WhatsApp available on the phone; a sent request
is not proof that the phone has answered. Replies become available only after
their matching captured chunk has been imported. A silent phone times out after
45 seconds, with a 30-second retry cooldown; it is not marked exhausted. A phone
that reports no more accessible history stops further requests. Navigation,
disconnect and close cancel pending request work; late replies still import
safely. The owner confirmed older messages loading from the phone on 17 October
2026. Recovery across phone/network failures still needs deliberate live checks.
Chat search matches the displayed chat name (or number for unnamed chats).
The conversation header’s search button searches saved message text and captions,
including older cached messages. It shows the newest 100 matches; narrow the query
for more. Select a result or click a quoted message to highlight the original.
**Back to previous view** restores the earlier timeline position and keeps the draft.
Search stays local to the encrypted archive; missing, deleted or expired originals
are reported as unavailable, without automatically requesting phone history.
Cached chats remain
available after Disconnect and can be reopened with `--offline`.

Synced phonebook names are stored separately from sender profile names and take
priority for other contacts' chat titles and group sender labels. Your own chat
uses your WhatsApp profile name followed by **(You)**, falling back to **You**
when the profile name is unavailable. The profile name remains available offline.
Contact renames/removals update
existing chats too. On the first connection after upgrading an older archive,
Heya requests a contact snapshot to recover names the older build did not save;
no new QR login is required. This uses contacts WhatsApp supplies to linked
devices, rather than reading a desktop or phone address book directly.

Conversation headers use a larger, vertically centered name, followed by **online**
or **last seen … at …** when WhatsApp supplies it. Unknown or private presence,
groups and self-chat show only the name. Presence is transient and subscribed
only for the open direct conversation; it clears on disconnect. The header uses
plain text and updates independently of message rendering. Synthetic lifecycle
and layout tests cover it; live-account availability still needs verification.

Saved call events appear as voice/video/missed-call cards and contact-list
previews, including calls synchronized from the phone. See [call events](docs/call-events.md)
for supported inputs and validation limits.

The phone button in a direct conversation starts a voice call. Incoming voice
calls have Answer/Decline controls; an active call has mute and hangup controls.
Linux reuses GStreamer device selection and volume settings; macOS uses native
AVAudioEngine/Core Audio. The pinned SDK supplies signaling, encryption, codecs
and media transport through its public calling API. Devices open only when
placing or answering a call and close when it ends or Heya closes. Group/video
calling, call waiting and ringtones are outside this first slice. See
[audio calling](docs/audio-calling.md) for tests and hardware/account validation limits.

Contact, group and account avatars load on demand for visible chats and the open
conversation. Small thumbnails stay in the encrypted archive for offline use;
downloads and decoding run off the UI thread. Picture-change/removal events
invalidate the cache, and missing or private pictures keep the initials fallback.
The owner has confirmed live sidebar pictures appearing. The collapsed sidebar
uses the same pictures. Incoming group sender runs now show participant photos
through the same bounded cache, including people without a private chat. Profile
names have a `~` prefix; saved contact names do not. Group sender photos are covered
by synthetic archive/UI checks; live-account confirmation remains open. Full-size
profile-picture viewing is not implemented yet. The demo uses synthetic geometric
images and never downloads account pictures.

Unread badges now consume phone/linked-device read receipts, synchronized read
ranges and history totals. On the first connection after this upgrade, Heya
requests a read-state snapshot to repair counts saved by older builds. Delayed
receipts preserve newer unread messages. A chat manually marked unread on another
device shows a dot when it has no unread messages. Reading the latest messages in
an active, visible conversation clears the local badge and queues a private read
update for WhatsApp and other linked clients. Reads made offline persist in the
encrypted archive and retry after reconnection at their original message position.
Inactive/minimized windows and scrolling through older history do not clear newer
messages. This synchronizes your devices. Sender-facing read receipts and voice
played receipts now have a separate durable queue. Settings → Privacy → **Send
read receipts** controls both for Heya, including groups; it defaults to on.
The account privacy setting is fetched before sending, and remains managed on
the phone. Turning Heya's switch off keeps private unread synchronization working
and does not backfill private reads when re-enabled. These receipt changes require
a current source build; previously published alpha.3 downloads are unchanged. Synthetic macOS checks do
not establish live receipt interoperability. Confirmed sends also advance the private read range on your other
devices, even when Heya's local unread badge is already zero. Queued/failed sends
leave it unchanged. See [read synchronization](docs/read-sync.md) for behavior and validation.

The Linux taskbar badge shows the number of unread chats, including chats manually
marked unread, and hides at zero. Search and sidebar filters do not affect it.
It uses the installed `org.heya.Heya.desktop` launcher and the Unity Launcher API
supported by Plasma and compatible docks. macOS uses its native Dock badge.
Muted, archived and locked chats are excluded. Demos and automated UI benchmarks
do not change the desktop badge.

Right-click a chat, use Shift+F10, or hover its row and choose the chevron for
**Mute for 8 hours / 1 week / always**, **Unmute**, **Archive / Unarchive**, and
**Lock chat**. Muting retains the chat's real unread count and displays a mute
indicator. **Archived** opens a separate list; **‹ Chats** returns to the normal
list. Folder changes save the current draft. Changing account state requires a
connection and is applied after confirmation, with no automatic retries for an
unconfirmed result. Incoming changes from other clients are synchronized too.

Locked chats are hidden from lists, search and forwarding destinations; an open
chat closes when its lock arrives. Heya only offers Lock: unlock the chat in
WhatsApp on your phone to show it again. It does not implement WhatsApp's secret
code or a separate local authentication flow. Offline use retains the last
synchronized state. See [chat management](docs/chat-management.md) for scope and
validation limits.

Conversations separate local calendar days with Today, Yesterday, weekday or full-date
chips. Same-day sender runs use canonical participant identities, so two people
with the same display name still receive separate sender headings. Runs retain
compact spacing, and bubble footers show the
time; copied transcripts retain the date. Labels refresh after midnight without
reopening the chat.

Message pin/unpin events appear as small centered notices naming the person who
made the change. Existing unsupported entries are repaired from their encrypted
envelopes at startup. Notices use the virtual timeline without message menus or
media controls. Sending pin/unpin actions and a pinned-message header remain
pending. `--demo --demo-chat 12` shows synthetic examples.

Outgoing footers distinguish Queued, Sending, Sent (server acknowledgment),
Delivered and Read. Group receipts say “some” because they do not prove every
member received/read the message. A send interrupted after network work starts
is **Unconfirmed · check phone**, and is never retried automatically. Failed
and unconfirmed text stays in the timeline; check your phone before deliberately
copying and sending it again. Only messages that were durably queued but never
attempted resume automatically on Connect.

The archive captures incoming supported message batches before the SDK's
acknowledgment and stages history chunks before importing them. It merges
history overlap, aliases, and supported sender edit/revoke forms. This does not
guarantee complete history: WhatsApp controls the history supplied, and some
SDK recovery events bypass the durability hook. Archive failures stop the
connection. Failed history chunks remain encrypted for future recovery.

Received link previews display the sender-supplied thumbnail, title, description
and domain in a native card. Clicking the card or a standalone URL opens the
system browser; focus the link and press Ctrl+C to copy its address. Heya does
not fetch websites to generate previews. Thumbnail decoding runs off the UI
thread and only nearby messages keep decoded pictures.

Business templates display their supplied title, body and footer, with URL and
copy-code buttons below the text. Reply, call, payment/flow and carousel actions
still need the phone and are labelled accordingly. Copying the loaded transcript
retains the complete text and action destinations.
Older placeholders, preview metadata and action rows are recovered from encrypted saved
envelopes on the next launch, without re-pairing or requesting history again.
Deleted, expired and edited messages are excluded from this repair; view-once
content remains unopened.

Received JPEG/PNG photos show an embedded thumbnail while downloading, then a
bounded native image with the caption underneath. Click a photo for a larger
viewer; use the zoom button or Space to switch between fit and zoom, and Esc to
close. Nearby photos download automatically; **Settings → Chats → Download
photos automatically** turns this off. Clicking an uncached photo downloads it
explicitly, and failed downloads offer retry. Originals and derived previews
stay in the encrypted archive, so downloaded photos also open offline. Switching
chats cancels work that is no longer needed; image work stays off the UI thread.
Older saved photo envelopes are upgraded locally on startup.

Sidebar photo previews use `Photo` or `Photo: <first caption line>`. Incoming
group previews include the sender's short name; unsaved profile names carry
`~`, while saved contact names take precedence. Closed chats' drafts override message previews.
These display summaries leave complete captions and copied transcripts intact.

Received documents show a native file card with a two-line filename, file type,
supplied page count and size. Embedded thumbnails appear immediately as a
first-page strip. Nearby cards use a separate high-resolution image when one is
available. For PDFs whose supplied image is missing or too small, Heya downloads
the authenticated PDF (up to 16 MiB) and renders just the first-page strip with
Poppler. On Linux this runs in a short-lived bubblewrap sandbox without network,
display or account-file access; input/output use pipes, with time and memory
limits. No PDF viewer or parser runs on the UI thread. Install the Linux runtime
tools yourself with `sudo pacman -S poppler bubblewrap util-linux` if missing.
Other files without a preview use a compact card. Separate preview downloads
are bounded to 1 MiB. Both paths share the authenticated media transport and
retain at most a 768×320 image, encrypted in the archive for offline use.
Switching chats cancels obsolete work; failed references have a one-hour retry
cooldown. Loading does not change card height. Unavailable, oversized, encrypted
or unrenderable PDFs retain the supplied thumbnail/icon. Scanned PDFs are limited
by the quality of the original scan. The PDF bytes are discarded after rendering.
Captions and full filenames remain in copied transcripts. Older saved envelopes upgrade on startup. Opening
documents inside Heya remains pending. The SDK
compatibility patch is tracked in [the upstream patch queue](patches/README.md).

Received voice notes and audio messages now have a compact native player: click
Play to download and play, click again to pause, click the waveform to seek, and
cycle the speed chip through 1×, 1.5× and 2×. Keyboard focus supports Enter/Space
on buttons and arrow keys on the waveform. Clicking during loading cancels it.
Only one clip plays at a time; changing chats or closing the conversation stops
playback. Downloaded audio is stored in the encrypted archive and works offline.
GStreamer reads bounded memory on a worker, without plaintext temporary files.
No playback thread is created at startup or while downloading. A thread starts
when audio bytes are ready and exits on stop, completion or failure. Pause keeps
the existing player for resume without polling for playback progress. Replacing a clip waits
for the previous player to finish teardown before starting another.
Audio is downloaded only on demand, with a 20 MiB limit. View-once audio stays
unopened. Sending voice notes is described above. Played receipts, automatic
next-clip playback and video playback remain pending. Codec and output failures are
reported with retry; real account/audio-device compatibility needs a deliberate
check after the synthetic silent-sink tests.

The attachment menu’s **Download** opens the Linux desktop’s native **Save As**
dialog and saves the original photo, audio, document, video or sticker. Settings →
Files selects its initial folder, defaulting to the system Downloads directory
plus `Heya` (appended once). One
explicit save runs at a time, up to 2 GiB, with progress, Cancel and Retry.
The Saved confirmation disappears after three seconds; failures stay visible for Retry.
Transfers stream to a private partial file outside the UI thread; the final file
appears only after authentication and integrity checks succeed. Existing files
remain intact on failure or cancellation. Changing chats does not redirect the
save. Cached original photos and audio can be saved offline; other attachments
require a connection. Saved files are ordinary, unencrypted files at the location
you choose and are not opened automatically. Linux requires a working
`xdg-desktop-portal` file chooser backend. See [downloads](docs/downloads.md) for
limits and validation.

The composer **+ → Photos / Documents** opens the native file chooser. Choose up
to 10 files, review the recipient and each caption, then press **Send**. The
selection is limited to 2 GiB in total. Photos support JPEG/PNG up to 20 MiB and
32 megapixels and retain their original bytes; choose Documents for other formats
or an image that should appear as a file. Captions support up to 1,024 characters
per attachment. Click a thumbnail/name to edit its caption or remove that item.
The existing text-composer draft is preserved independently. Upload progress is
nonmodal, so you can keep chatting and scrolling after Send.

Preparation, encryption, thumbnail decoding and streaming upload happen away
from the UI thread. After each upload, the envelope is saved in the encrypted
outbox before that item leaves the selection. A failed upload keeps the remaining
items and captions for **Retry**, without resending items already queued. Cancel
keeps unsent items available; Close discards the unsubmitted selection. Review
selections are session-only. After queueing, ordinary delivery receipts and the
existing no-automatic-resend rule apply. Closing Heya cancels outstanding uploads;
files already queued remain in the durable outbox. Upload workers exit after their
work; unnamed encrypted staging files disappear on close or process exit. See [sending attachments](docs/attachments.md)
for limits and validation. Clipboard photo/file paste, native X11/macOS file drops,
and original-photo Copy image now use this flow, including native Wayland file
drops through the pinned development backend. Video recording,
and an image editor remain pending.

Automatic photo and audio-player downloads are limited to 20 MiB and decoded photos to 32 megapixels. Chat images
are at most 640px on the long edge; the one open viewer uses at most 2048px.
Expired CDN references may require the sender to resend the photo. Automatic
media re-upload recovery, disk-cache eviction,
and video playback remain pending. Other unsupported attachment types retain placeholders.
View-once content is not opened. Administrator deletion variants, chat-settings synchronization,
and full local account/data removal remain pending. Disappearing-message expiration is
basic and depends on timer metadata supplied with messages/history.
No live pairing or message sends have been performed by automated validation.

Hovering a message reveals React, Reply and Forward. Incoming controls sit to
its right; outgoing controls sit to its left, and controls that cannot fit are
hidden. React opens a searchable emoji picker; selecting your current reaction
removes it. Reply prepares a quoted composer draft; sending remains explicit.
Forward opens a searchable list of existing writable chats and requires choosing
a recipient and pressing Forward. Received photos/audio retain their media
references; business cards forward as their readable text. View-once, expired
and deleted sources cannot be used. Reply drafts and pending operations survive
restart in the encrypted archive. Reactions do not become separate chat bubbles.
A chevron in the hovered bubble opens a shared menu and quick-reaction strip.
Reply, React, Forward, Copy text/caption and attachment Download are connected.
Pin, Star, Report and Ask Meta AI remain visibly disabled. The menu supports
keyboard navigation, Escape and outside-click dismissal, and fits short windows
with a scrollable list. Attachment classification for older messages is recovered
once from encrypted envelopes on the archive worker, without fetching media.
Synthetic tests cover these flows; real account interoperability remains to be
checked deliberately. Aggregate reaction snapshots embedded in history, a
participant list for reactions, multi-recipient forwarding and expired-media
re-upload recovery remain pending.

Right-clicking a message opens a ZapFast-style integrated quick-reaction/menu
card. Reply, Forward, Select, Copy text, eligible own-text Edit, Delete for
everyone, Delete for me, Message info and Copy message ID are connected. Own
text edits are offered for 15 minutes and sender deletion for 48 hours, matching
the pinned reference; the archive and transport recheck eligibility. Edit uses
a separate dialog so an unsent composer draft is preserved. Deletion requires
an explicit confirmation. Selection supports forwarding to one chosen chat,
copying and deleting for me, with up to 100 selected messages.

Received photo/document caption edits preserve attachment metadata, cached media
and in-flight transfer requests. Quotes and forwards use the updated caption;
older edits and history replay cannot undo it. Archive upgrades recover media
cards damaged by older caption handling, but captions those releases discarded
cannot be reconstructed. This does not add outgoing media-caption editing.

Edits and sender deletion use the existing durable outbox and apply locally on
server acknowledgement. Unconfirmed operations are labelled and are not blindly
resent. Delete for me hides locally and queues idempotent linked-device app-state
sync; history replay cannot restore hidden content. Incoming sender deletions
show **This message was deleted** in the bubble and last-message preview, even
when the original never reached Heya. Startup repairs previously saved delete
events rejected by the older recipient-key check. Receipt details show only
actual supplied times; historical recipient evidence may be unavailable. The
SDK's validation can reject unsupported edit contexts, including community
announcement edits requiring an original message secret. These failures retain
the original content. Live edit/delete interoperability still needs deliberate
manual validation; automated tests never modify the owner's account.

On Linux the session is under `$XDG_DATA_HOME/heya/default-v2/session.db`
(normally `~/.local/share/heya/default-v2/session.db`), with `archive.db` beside
it. On every platform, one random 256-bit master key is stored under service
`org.heya.Heya`, entry `master-v1`, using native macOS Keychain or Linux Secret
Service. One keyring read supplies both databases' distinct keys, derived with
HKDF-SHA256 and separate session/archive contexts. The master key is stored as
versioned hex text for desktop-keyring compatibility. An empty entry left by a
failed initial write can be recovered only if neither database nor its journal
files exists. Both databases use SQLCipher, private
file permissions and the same process lock. A locked or unavailable keyring
produces a retryable error;
Heya never falls back to plaintext or replaces a missing key for an existing
database. Only one process can open this account. Do not remove either database
or its key independently. A revoked linked session is retained until an explicit
**Link again** confirmation. An interrupted shutdown blocks reconnection until
Heya is restarted, so two protocol clients cannot share the same credential store.

This pre-release storage change requires QR pairing again. The former `default`
profile and its `session-v1`/`archive-v1` keyring entries remain untouched; there
is no migration and the new app does not read them.

Closing the window quits Heya: queued drafts/settings are saved, audio work is
cancelled, active file exports are cancelled, and the SDK flushes before the session lock is released. Published
audio, download and upload sources are cleared before waiting for SDK ownership to end; retaining
the audio channel value previously caused a 15-second cleanup timeout after closing.
There is no background/tray mode. Cleanup failures now log a fixed stage or
persistence flags without account identifiers.

## Offline demo

```sh
cargo run --locked -- --demo
```

The demo uses synthetic conversations and keeps changes in memory. It does not
open account files or connect to WhatsApp. Try switching chats with a draft,
searching the chat list, the Unread filter, Settings, selecting text in a
message, copying the loaded transcript, and sending a local demo message.
Enter sends by default; Shift+Enter inserts a newline. Turn off **Settings → Chats → Enter sends** to use Enter for new lines and Ctrl+Enter to send.
The Timeline lab contains 10,000 messages, loaded in pages of 100.
The Message cards chat uses synthetic link previews and delivery actions;
`--demo --demo-chat 7` opens it directly.
`--demo --demo-chat 8` shows synthetic dates and message-run spacing.

Light is the default. **Settings → Appearance → Theme** offers Light, Dark, Follow system and the eight named ZapFast palettes. `--light` / `--dark` override the starting theme; use `--size 800x600` for a smaller window. Below
620 logical pixels the sidebar and conversation become separate views. To render
a screenshot from the application and exit:

```sh
cargo run --locked -- --demo --screenshot /tmp/heya-demo.png
```

For a repeatable desktop responsiveness measurement, run
`cargo run --release --locked --features slint/unstable-wgpu-30 -- --demo --ui-benchmark`.
The extra feature enables Vulkan rendering timing callbacks. It uses 500 synthetic
chats, exercises update bursts, resizing and maximize/restore, then loads all
10,000 synthetic messages for scrolling, resizing and composer growth. It prints
timings and exits after 27 seconds. It never opens an account or
captures pixels. See [development notes](docs/development.md) for scope and results.
The workload includes synthetic avatar thumbnails; set
`HEYA_BENCHMARK_NO_AVATARS=1` for a benchmark-only comparison with pictures disabled.
Set `HEYA_BENCHMARK_MESSAGE_CARDS=1` to replace the timeline with alternating
link previews and business actions, including synthetic thumbnail textures.
Use `HEYA_BENCHMARK_PHOTOS=1` for received-photo rows and worker-published pixels;
`--demo --demo-chat 9` provides the small photo layout fixture.
`--demo --demo-chat 10` shows received audio; demo playback uses a silent output.
`HEYA_BENCHMARK_AUDIO=1` measures 10,000 audio rows with silent GStreamer playback.
`--demo --demo-chat 11` shows synthetic document cards, including a file without a
thumbnail. `HEYA_BENCHMARK_DOCUMENTS=1 scripts/ui-offscreen.sh --ui-benchmark`
measures 10,000 document rows using a temporary encrypted archive and the real
thumbnail worker. It neither reads nor downloads personal documents.

Settings are available from the gear button or **Ctrl+,**. The avatar opens the
Account section; **Ctrl+B** collapses/expands the chat list and **Ctrl+N** opens New chat.
The collapsed list keeps avatars, unread badges and chat selection available.
Theme, font, Enter sends, automatic photo downloads, doodles and the shortcut hint apply immediately. Live
runs save these non-account preferences to the platform config directory
(`~/.config/heya/settings.json` on Linux); demo runs keep them in memory only.
Settings use the reference's Appearance, Chats, Notifications, Privacy, System,
Account, Files and About sections. Unfinished applicable settings are labelled
**Not implemented yet** and cannot change account state. Account connect/disconnect
and confirmed unlink/relink are available in Settings. The additional contact filters and media composer
controls are present but disabled until their behavior is implemented.
Omarchy-specific integration is deliberately excluded.

Automated UI checks run on an isolated virtual display by default:

```sh
scripts/ui-offscreen.sh --screenshot /tmp/heya-demo.png
scripts/ui-offscreen.sh --screenshot /tmp/heya-wide.png --size 3756x2033 --light
HEYA_TEST_RENDERER=winit-skia-opengl scripts/ui-offscreen.sh --framebuffer /tmp/heya-rendered.png --light
scripts/ui-offscreen.sh --ui-benchmark
scripts/ui-offscreen.sh --close-check
HEYA_BENCHMARK_NO_WALLPAPER=1 scripts/ui-offscreen.sh --ui-benchmark
```

The runner uses Xvfb and Skia's CPU rasterizer, private temporary application
directories and synthetic data. It opens no windows on the working desktop.
Xvfb and xauth are already installed on the development machine. These runs test
layout and application behavior; their frame timings do not establish normal
Wayland/Vulkan performance. Skia CPU frame counters are unavailable, and Xvfb has
no window manager for maximize/restore. Visible compositor/performance checks require prior
agreement. The direct screenshot command above opens a real desktop window.

The benchmark-only `HEYA_BENCHMARK_NO_WALLPAPER` flag disables doodles for a
comparison; it does not change saved preferences. Run comparisons sequentially,
after compilation has finished, on the same renderer and display setup.

`--framebuffer` captures the actual synthetic window inside Xvfb, using the
already installed `xdotool` and ImageMagick. It then terminates that disposable
demo process. Use it for graphics-backed images: Slint's `take_snapshot` can
omit cached GPU textures when re-rendering them into its CPU snapshot surface.
The default `--screenshot` uses Skia CPU and avoids that capture limitation.

`--close-check` sends native window-close requests on the private X server and
checks complete process exit before reopening. It covers the demo and normal
startup with empty isolated data directories (no account or keyring access).
SDK tests separately exercise encrypted session release/reopen while an audio
source is published, and cancellation of an in-flight audio download.

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

Tests use synthetic data and temporary databases. They cover draft/send routing,
history ordering/replay, alias and tombstone merging, interrupted sends, durable
inbound hooks, receipt mapping, Slint callbacks/layout and encrypted storage.
They also exercise the real outbox worker with a synthetic transport, session
reset/archive preservation, and draft recovery after failed navigation. The
desktop-keyring test is opt-in because it needs an unlocked Secret Service:

```sh
cargo test --locked --bin heya storage::tests::desktop_keyring_encrypts_and_reopens_a_disposable_database -- --ignored --exact
```

It creates and removes a disposable keyring entry and temporary database; it
does not use the WhatsApp session. These checks do not establish live pairing,
delivery or network recovery; those still require deliberate account testing.
Timeline tests cover bounded component creation with 10,000 messages and stable
message anchors through resizing, edits, history prepends and removal.
Video playback, attachment staging and cross-message drag selection remain
pending. Native IME, clipboard and scroll performance still need the M0 checks.

## Start here

1. [Product scope and UI specification](docs/product-and-ui.md) — visual direction,
   what to improve, and how to tell whether the port is successful.
2. [Architecture](docs/architecture.md) — runtime, protocol boundary, persistence,
   media, and desktop integration.
3. [Implementation roadmap](docs/roadmap.md) — ordered milestones and acceptance gates.
4. [Upstream audit](docs/upstream-audit.md) — exact revisions, source entry points,
   and integration findings.
5. [Development preparation](docs/development.md) — local environment and build plan.
6. [Third party notices](THIRD_PARTY_NOTICES.md) — retained upstream attribution.

The first implementation milestone remains the offline Slint feasibility prototype:
selectable messages, a long conversation, image/file paste, and a local video.
The current demo begins this milestone; media and richer clipboard integration
remain open. The live text/archive slice has started alongside those checks at the
user's request; it does not mark the earlier acceptance gates complete.

## Preparation checks

Run with Python 3.11 or newer:

```sh
python3 scripts/check_environment.py
python3 scripts/check_environment.py --media
```

The checks inspect installed tools and libraries. They do not install packages,
connect to WhatsApp, or read account data. Pinned reference revisions are in
[prep/upstreams.json](prep/upstreams.json); the proposed Cargo dependency block is
[prep/dependencies.toml](prep/dependencies.toml).

The base and media environment checks pass on the development host, including
MP4 demuxing and automatic audio output. Slint's language server and viewer are
installed at version 1.18.1; they predate the current development toolkit pin.
Use tools from the pinned Slint revision when testing new toolkit APIs. See
[development preparation](docs/development.md) for the verified setup status.

The user manages system package installation; the development guide distinguishes
installed prerequisites from optional tools. Dependencies are selected for
technical fit, with upstream notices retained as routine project housekeeping.

## Measuring the archive-to-UI path

The ordinary synthetic renderer benchmark is separate from the production-path
benchmark. Run the latter after `cargo build --release --locked`, without other
build/test jobs running:

```sh
HEYA_BENCHMARK_ARCHIVE=1 HEYA_TEST_RENDERER=winit-skia-opengl scripts/ui-offscreen.sh --ui-benchmark
```

It creates a disposable SQLCipher archive with 500 chats and a 10,000-message
conversation, then uses the real messaging worker and coalesced Slint handoff.
It measures archive queue/lock wait, database jobs, query/projection, snapshot
publication, UI application, ingest-to-next-render and navigation-to-next-render.
It also reports full/append timeline projection, transcript formatting and cached
timeline reuse separately, so skipping UI work cannot hide archive rebuilding.
Phases cover single arrivals, unrelated chats, twenty-message bursts, actual
paging to 10,000 loaded rows, wheel scrolling and navigation. Issued, applied and
rendered counts expose batching/coalescing; latency samples describe observed
snapshot generations, not every individual message. Cold transitions, maxima
and missed updates are retained in the output. No live account or protocol
connection is opened. Instrumentation is enabled only on that temporary archive.
These are application CPU and event-loop measurements; compositor presentation,
GPU completion, actual device input and phone/network latency are not measured.
