# One-to-one audio calling

The direct-chat header has a phone button. Incoming one-to-one audio offers show
the caller, cached photo, Answer and Decline. The active panel shows connection
state, elapsed time, mute and hangup. No microphone/device opens while idle or
merely ringing. A missing microphone is reported through the existing dialog;
the incoming offer remains answerable until it expires. Group/video calls, call
waiting, ringtones and OS call integration are separate work.

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
