# One-to-one audio calling

The direct-chat header has a phone button. Incoming one-to-one audio offers show
the caller, cached photo, Answer and Decline. The active panel shows connection
state, elapsed time, mute and hangup. No microphone opens while idle or merely
ringing. A missing microphone is reported through the existing dialog;
the incoming offer remains answerable until it expires. Group calls, call
waiting and OS call integration are separate work.

Two-way direct video calls share this controller and audio lifecycle; see
[video calling](video-calling.md) for camera ownership and codec validation.

Outgoing calls play a gentle repeating ringback after the SDK sends the offer;
incoming offers play a distinct chime. Both are original synthesized sounds,
with no downloaded assets or additional runtime dependency. They use the selected
audio output and volume through the same finite helper as received audio.
Answering stops and reaps the ringtone before microphone setup. Ringback stops
on acceptance/connection; ending, declining, expiry, disconnect and close stop
and reap the relevant helper too. No sound worker remains at idle. Automated
checks use a silent output rather than the owner's speakers or microphone.
Queued answers/failures take priority over starting ringback, and a failed
answer does not automatically restart the incoming ringtone.

Outgoing microphone/permission/device checks complete before the call panel opens.
Failed checks show an audio error popup without a preparing/ended call dialog.
Once audio is ready, Calling appears while signaling starts, rather than hiding
the network wait. Initial signaling failures close the panel and show the error
popup. The header shows Checking audio during the brief readiness check, with
repeated starts ignored.

An outgoing audio check has a Cancel button and an eight-second deadline covering
device discovery, device opening and the first microphone frame. Failure restores
the header and shows an audio error, without starting signaling. During setup and
an active call, five seconds without a complete microphone frame fails the audio
operation too; a silent or muted microphone still supplies PCM. These deadlines
run in the parent, so a blocked platform call cannot bypass them. The existing
pipe-close/kill/reap path releases the helper before publishing the result.

The call panel handles ringing,
connection progress and later failures normally. Incoming offers remain visible
before answering, without opening the microphone merely to ring.
Mute is enabled once the SDK returns a call handle; hangup is available during
network setup too.

## Protocol and audio ownership

`protocol_calling` uses the pinned whatsapp-rust `voip` public facade, with no
new library patches. The SDK owns offer/accept/reject/terminate negotiation,
encryption, MLOW/Opus encoding/decoding and native relay transport. Heya supplies
960-sample microphone frames (16 kHz, mono, signed 16-bit PCM) and consumes decoded
speaker PCM. Platform code does not implement WhatsApp media protocols.

One controller owns setup, mute and teardown. Ordered bounded SDK control events
preserve acceptance during setup; a companion's `busy` rejection is not treated
as an explicit user decline. Other-device completion and disconnect clear stale
incoming offers. Captured offer IDs prevent queued controls acting on a later
call. Direct conversation identity/lock/read-only state is rechecked before
signaling, including aliases changed while microphone permission was pending.

Audio runs in a finite helper subprocess, like existing playback/recording.
Anonymous pipes carry bounded PCM queues. The microphone must produce a frame
before Heya starts or accepts signaling. Hangup bypasses the ordinary control
queue, and setup/answer deadlines are bounded. Ending, disconnecting and closing
stop device work and SDK media together. Closing the parent pipe terminates the
helper even if device startup blocks; the parent waits for exit or kills/reaps it.
No call-audio process, dedicated device thread or PCM queue remains at idle.

Linux uses one duplex GStreamer pipeline with existing input/output selection and
gain preferences. `webrtcdsp`/`webrtcechoprobe` provide echo cancellation when
installed (Arch: `gst-plugins-bad`); otherwise use headphones. Both branches use
16 kHz mono PCM, finite queues, and current-time speaker timestamps so time spent
ringing does not make all subsequent audio late. Speaker preroll cannot block
microphone startup.

macOS uses AVAudioEngine with Core Audio device selection, native voice processing
and microphone permission in the application bundle. It does not load GStreamer.
No-default-feature Linux builds retain signaling/history but report call audio
unavailable. Idle call state and the active timer update independently of the
conversation transcript and image models.

## Reproduction and validation limits

Ordinary offline tests cover repeated outgoing calls, incoming answer/decline,
mute, remote end, absent microphones, stale controls, expiry, disconnect and
closing during active calls/setup. The fake signaling service exchanges PCM
through the real finite synthetic helper and never contacts an account. Adapter
tests construct an unspawned synthetic SDK bot to check offer/control routing.
Shutdown tests include the calling source in the SDK session-release fence.
Toolkit tests check panel/text containment at 520/800/1200 px, actual controls,
rapid repeated actions and keyboard focus without sending a hidden draft.
Button-to-controller tests click the actual header and run the finite audio
helper, covering absent microphones, setup failure, visible Calling during slow
network setup, mute, hangup and dismissal. A connection-state transition or an
unfinished voice message produces a popup instead of silently rejecting a click.

For the real Linux pipeline, use the unit-test executable produced by
`cargo test --release --locked --bin heya --no-run`:

```sh
HEYA_TEST_BINARY=/absolute/path/to/target/release/deps/heya-TEST_HASH \
  scripts/ui-offscreen.sh --call-audio-check
```

This uses `audiotestsrc`, `fakesink`, and DSP/probe when available, with no physical
microphone, speaker output or account. Linux CI also runs it explicitly.
Static synthetic call-panel screenshots can be reproduced with
`HEYA_TEST_CALL_STATE=1` (incoming), `5` (active), or `6` (ended) and the ordinary
offscreen screenshot wrapper. This hook is only in the offline demo path.

`HEYA_BENCHMARK_CALL_UI=1 HEYA_TEST_RENDERER=winit-skia-opengl
scripts/ui-offscreen.sh --ui-benchmark` measures the active panel with cached
caller pixels and one-second duration updates during background snapshot bursts
and resizing. It does not start transport or device audio; modal call controls
block ordinary conversation scrolling during this run. Compare ordinary UI
scrolling with the same command without `HEYA_BENCHMARK_CALL_UI`.

On 21 October 2026 the Linux suites passed 287 application tests with media and
269 without it, plus strict all-target Clippy in both configurations. The complete
toolkit test was rerun after the final focus and button-spacing adjustments.
The real silent GStreamer duplex check passed, and the offscreen native
close/reopen check found no idle media thread and exited in 40–47 ms across
four synthetic demo/no-session launches. Local unit executables used release
dependencies with application optimization and LTO disabled; application builds
and UI timing runs use the normal optimized release profile.

These tests establish lifecycle, bounds, PCM flow and UI behavior. They do not
establish real microphone/speaker quality, acoustic echo cancellation, macOS
device/permission interoperability, or successful calls to an official client.
Those require deliberate manual calls with suitable hardware. Heya never places
an account call as part of its automated tests.

Call diagnostics record fixed setup stages (offer/answer sent, peer accepted,
relay ready) and numeric media counters when the call closes. They contain no
phone numbers, call IDs, packet contents or raw SDK errors. Calling is shown
while signaling starts; it does not establish that a recipient is ringing.

An outgoing call also logs one `Heya call trace:` line when its setup ends:
connected, declined, ended, closed, failed, hung up, offline or dropped. The
pinned SDK reports a callee device's offer receipt (when WhatsApp shows
"Ringing") and its `enc_rekey_retry` call-key re-request only as plain receipts,
and it does not re-send the call key. Heya's receipt handling ignores both. So
`protocol_call_trace.rs` reads stanzas through the SDK's public raw-node
forwarding lease and event subscription. These are held only from just before
the offer until the setup outcome. A sent-node waiter captures the offer's
shape. The line reports:
- how long the SDK took to prepare the offer;
- how many devices received an encrypted call key, how many of those used a new
  Signal session, and whether a privacy token was attached;
- the server's ack, or its error code;
- distinct ringing devices and call-key requests, with first-arrival times;
- up to four callee preaccept, accept, reject or terminate replies, with their
  protocol reason;
- the setup duration.
Device JIDs are counted in memory only. Reasons outside short ASCII protocol
words are written as "other". Synthetic stanzas test classification and the
summary. Manual calls have exercised ringing receipts and preaccept replies;
call-key retry behavior remains unverified.

A voice or video call that Heya places is recorded when its session is released,
in `protocol_call_log.rs`. Signaling failures before the offer are not recorded.
The record goes into the call's chat first, through the same path as synced
call logs. Heya then publishes the same `CallLogRecord` to the account's
`call_log` with the patched SDK's `Client::send_call_log` (see
[the patch notes](../patches/README.md)), so the phone and other companions can
list it. Result codes follow WA Web's VoIP mapping:

| Outcome | Call result | Callee result |
| --- | --- | --- |
| Answered (with duration) | connected | connected |
| Declined | rejected | rejected |
| Unanswered, or ended by the callee or server | missed | missed |
| Hung up before answer | cancelled | missed |
| Offline or media failure before answer | unavailable | missed |

The record also carries the video flag, the offer time and the literal
`isIncoming = false`. A decline that reaches Heya just after the SDK closed the
call is still recorded as rejected. Heya shows an outgoing missed call as
"No answer"; that also applies to outgoing calls synced from the phone. The
account write is bounded to 10 seconds and is not retried, so a call that ends
while offline appears only in Heya. One `Heya call log:` line reports whether it
was published. A later synced record for the same call replaces Heya's local one.
Queued SDK failures take precedence
over the sticky end notification, so failed relay setup is reported as an error
rather than an ordinary hangup. Synthetic tests cover both notifications becoming
ready together and distinguish transport failure from a normal termination.

Audio failure regressions also stall a synthetic helper before device opening
and midway through a PCM frame. The playback fixtures stall startup or an active
player; the recording fixture reports status without capturing samples. These
exercise parent deadlines, cancellation, retry and helper reaping independently
of physical audio devices. They do not diagnose or repair the host audio server.
