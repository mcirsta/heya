# Saved call events

Heya displays voice/video call events in the conversation and uses the latest
event for the contact-list preview and ordering. Missed calls have a distinct
label; conversation cards include direction, available duration and outcome.
The cards use a fixed, small Slint surface, text and an SVG icon. They have no
call button, message menu, decoder, timer or media service.
The owner's 21 October WhatsApp Web screenshot supplies the sidebar reference;
the implementation uses the pinned SDK event contracts and protobuf schema.

The unchanged pinned whatsapp-rust public APIs provide three input paths:

- `CallLogSync` in the existing coalesced app-state mailbox, including calls
  made or answered on another device. `from_me` comes from the SDK's creator
  identity resolution; the inconsistent wire `is_incoming` flag is ignored.
- `HistorySync.call_log_records` through Heya's durable captured-history worker,
  processed in batches of 100. Heya deliberately does not also subscribe to
  the SDK's transient per-record `CallLogHistory` burst.
- Ordinary `call_log_messsage` envelopes (the SDK spelling), including supported
  wrappers. Previously unsupported retained envelopes are repaired on startup.

Finished missed/answered-elsewhere signalling can create a provisional entry
before the phone's record arrives. It does not invent an audio/video type.
The authoritative record fills it in. Placing/answering calls is implemented
separately; see [audio calling](audio-calling.md).

Calls share encrypted timeline storage and stable identities. Repeated call IDs
update an existing row, scoped to its canonical chat and sender. The event uses
the call's own start time rather than mutation arrival time; older app-state
replays cannot replace a newer result. History cannot overwrite a newer synced
record. PN/LID merges reuse the existing archive alias handling. Outgoing direct
calls require one identifiable remote participant; group calls require a group
JID. Ambiguous destinations, absent identities/IDs and invalid start times are
omitted rather than attributed to an arbitrary chat.

Call entries do not increment message unread counts, enter the outbox, send
message receipts or become phone-history/read-range message keys. The existing
draft, mute/archive/lock, sidebar and virtualized timeline paths are reused.
On the first connection after this upgrade, the existing state-recovery worker
requests a `Regular` snapshot to recover call logs previously ignored by Heya.
Consumed old history chunks no longer retain their compressed data, so this
does not promise recovery of calls the phone no longer supplies.
Call-log deletion synchronization and legacy history stubs without a call
record/envelope remain outside this first slice.

Automatic tests use synthetic records and compressed history only. They cover
direction, outcomes, unknown metadata, aliases, replay, updates, reopening,
repair, summary/draft ordering and isolation from messaging actions/receipts.
Offscreen layout/benchmark checks use `scripts/ui-offscreen.sh`, demo chat 15
and `HEYA_BENCHMARK_CALLS=1`. These establish local storage/UI behavior, not live
phone interoperability or native macOS validation. No live calls are generated.

## Linux validation — 21 October 2026

The final suites passed 275 tests with default media and 257 without it, with
seven native/opt-in checks ignored in each ordinary suite. The encrypted protocol
storage integration test and strict all-target Clippy passed. The normal saved
session reopened connected and completed its initial call-state snapshot.
On 21 October 2026, the owner confirmed that a real call appeared both in the
sidebar as “Voice call” and as a conversation entry.

An initial implementation expanded the generic notice component and regressed
ordinary chats. Two sequential runs and a reverse-order repeat found scrolling
at 15.14–15.29 ms render CPU p95 against a 12.42–12.98 ms baseline. Separating
`CallEvent` and using its fixed height removed the extra ordinary-notice
measurement bindings. No toolkit patch or per-message workaround was added.

| Final optimized offscreen measurement | Ordinary messages | 10,000 call events |
| --- | --- | --- |
| Scrolling render CPU p95 | 13.04 ms | 11.34 ms |
| Resizing render CPU p95 | 12.97 ms | 8.85 ms |
| Unchanged snapshot application p95 | 0.11 ms | 0.09 ms |

The final comparison ran sequentially without concurrent compilation, using
private Xvfb and Skia/OpenGL with Mesa fallback. Cold renderer initialization
is retained separately in the raw measurements; these warm-phase numbers do
not measure physical compositor presentation, GPU completion or network latency.
The complete [measurements](../prep/validation/calls-20261021/measurements.json)
retain the initial regression and both before/after orders.
