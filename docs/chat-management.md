# Chat management

Heya synchronizes mute, archive and lock state from history and app-state updates.
The sidebar menu is available through right-click, Shift+F10 and the hover
chevron, including the compact sidebar. It offers timed muting (8 hours or one
week), indefinite muting, unmute, archive/unarchive, and Lock.

Muted chats retain their real unread messages and remain in the current folder;
the expanded sidebar shows a trailing mute icon and the compact sidebar shows
a small mute marker beside the avatar. Both keep the unread count visible;
the taskbar/Dock badge excludes them while their mute is active. Expiry is
evaluated using the existing archive refresh interval, so the badge can take up
to roughly 30 seconds to reflect an elapsed mute. Archived chats have their own
list and are excluded from that badge. Filtering and folder selection do not
change the global count. Opening a different folder saves the current draft.
The account-wide Keep chats archived preference remains managed on the phone;
Heya applies the archive/unarchive state updates it receives from WhatsApp.

Locked chats stay out of normal and archived lists, sidebar search and forward
destinations. An arriving lock clears an open conversation and its photo/message
overlays. New text, attachment, reply and read requests are rejected for locked
targets, while messages, drafts and unread state remain in the encrypted archive.
Previously queued sends are not undone by locking a chat.

As selected for this increment, unlocking is done in WhatsApp on the phone. Heya
does not verify WhatsApp's secret code or provide a separate authentication
system. This is synchronized chat visibility, not a new encryption boundary.
Cached state may be outdated while offline, as with other linked-device state.

## Storage and synchronization

Schema 16 adds one indexed settings table, independent of listed chats. Metadata
for a previously unknown JID does not create a sidebar entry. Per-setting wire
timestamps keep stale or partial history from undoing newer changes; PN/LID
reconciliation merges the same state. History without a timestamp seeds settings
until a versioned app-state update is known. No raw message/media data is needed
to compute these flags.

The existing bounded app-state mailbox coalesces updates by chat and setting.
Fresh/unclean sessions request the regular-high mute collection along with the
existing regular-low and contact collections, because the SDK may already have
advanced a cursor before Heya saved an event. Failed synchronization preserves
saved state and its recovery marker. State is persisted offline and rechecked
when the session resumes. A connection that attempted an explicit change leaves
the marker dirty even after normal shutdown, so the next connection repairs any
cursor/write gap from cancellation. This uses one connection flag and does not
retain or automatically replay the requested action.

Explicit actions use the pinned SDK's public `send_app_state_action` API and its
registered MUTE, ARCHIVE and LOCK_CHAT schemas. Heya supplies the timestamp so
the acknowledgement and subsequent echo refer to the same version. Archive
actions include the cached tail range when available. No SDK patch or ZapFast
application implementation is used.

An action owns one future in the existing messaging loop. Drafts and other
commands continue while it waits. Completion releases that future; there is no
dedicated thread or new idle worker. Disconnect, source replacement or window
closure cancels the operation and releases its client ownership. The normal
shutdown fence clears the source watch before waiting for the SDK store to close.
At submission it captures the current client and marks the connection notification
seen. Only later connection changes cancel the action; an unread initial Connected
notification must not reject a valid click as offline.
An unconfirmed action does not optimistically change saved state or retry itself;
subsequent authoritative updates can still report a change that reached WhatsApp.

## Validation

Automated checks use synthetic SQLCipher archives, fake action sources, the real
SDK event bus, headless Slint input/layout and the existing synthetic SDK shutdown
test. They cover timestamp ordering, missing values, alias reconciliation,
migration/restart, mute expiry, folder filtering, badge exclusions, lock guards,
retained drafts/unread state, menu containment, mouse/keyboard activation, mute
indicator containment beside unread/call previews, late connection publication,
delayed mute/unmute completion, cancellation and source release.
These do not establish live-account interoperability or
native macOS interaction.

The archive benchmark can additionally populate 1,500 flags and 250 muted chats:

```sh
HEYA_BENCHMARK_ARCHIVE=1 HEYA_BENCHMARK_CHAT_SETTINGS=1 HEYA_TEST_RENDERER=winit-skia-opengl scripts/ui-offscreen.sh --ui-benchmark
```

It runs through the real database, messaging worker and UI handoff on a private
X server. Application/render callback timings do not measure a desktop compositor
or account delivery. Recorded results are in [development notes](development.md).
