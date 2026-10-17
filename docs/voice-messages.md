# Sending voice messages

An empty composer offers the microphone button. Click once to check the available
inputs and record. With no microphone, Heya opens a “Microphone not found” dialog
explaining how to connect/check an input. “OK, got it”, Enter or Escape dismisses
it and returns to the composer; it creates no recording pipeline. The check is
repeated on the next click, so attaching a microphone does not require a restart.
Output-monitor sources are excluded. The strip
replaces the input field and shows elapsed recorded audio and a waveform, with
Discard, Pause/Resume and Stop. Stop finalizes the Ogg stream and releases the
microphone before review. Review offers playback, discard and explicit Send.
The text draft remains independent. A reply selected before recording is kept;
cancelling that reply also clears it from the pending voice note. New message
actions are disabled while recording/reviewing to avoid changing its context.

Changing chats or opening Settings/New chat/Account discards an unsubmitted
recording and stops capture. On connection loss, capture finishes into review;
sending requires reconnection. The destination captured at Start cannot become
a different recipient during send. Once Send hands the note to the transfer
service, its ordinary nonmodal progress, Cancel/Retry, stable message ID and
uncertain-delivery handling apply. A failed voice upload retries directly from
the transfer card. Review data is session-only; window close discards it, with
no automatic send or recovery of an unsubmitted recording.

Capture uses GStreamer in a short-lived helper process from the same Heya binary:
`enumerated microphone source → audioconvert → audioresample → S16LE/48 kHz/mono → opusenc →
oggmux → appsink`. Opus uses its voice preset at 32 kbit/s. Compressed Ogg bytes
stay in bounded memory; PCM is transient in the pipeline. Limits are 30 minutes
and 20 MiB. These are Heya policies. The waveform is bounded to 64 displayed bars,
with coalesced progress at 10 Hz. No per-message recording control or idle capture
worker is added. Stop/failure/cancellation tears down the pipeline and exits the
helper before completion is published. Preview uses a separate finite helper,
which exits before send/discard. This keeps GStreamer global pool threads out of
the parent for recording and preview. The existing Tokio runtime handles bounded
binary pipes; no extra persistent coordinator thread or daemon is created.
Cancellation has a bounded shutdown followed by kill/reap if necessary. Closing
the parent pipe also terminates the helper, including after a parent crash.
Startup does not open a microphone.

Explicit Send stages authenticated ciphertext into an unnamed file with the
SDK's Audio media type, using the existing streaming uploader. Its public
AudioMessage fields supply `ptt=true`, `audio/ogg; codecs=opus`, seconds, waveform,
streaming sidecar, media hashes/key and optional quote. The original Ogg is saved
in the encrypted audio cache in the same transaction as the outgoing message,
so a queued note is playable offline. No schema migration or new SDK patch is
required. The usual cache invalidation guards still cover edits/deletion/expiry.

The `media-gstreamer` feature gates capture and preview. Without it the microphone
is disabled. Missing Opus/Ogg/source plugins and inaccessible microphones produce
short errors, and no failed capture is sent. System packages are installed by
the owner; the needed factories are supplied by Arch's GStreamer base/good
plugins, with the normal PipeWire/PulseAudio or ALSA microphone setup.

Automated recording is synthetic: demo mode and tests use `audiotestsrc`, and
preview uses a silent sink. No automated test reads the real microphone or sends
an account message. The validation results, offscreen captures and responsiveness
measurements are recorded in [the UI audit](ui-source-audit.md). These do not
establish real microphone permissions, hardware routing or recipient playback;
those need the owner's deliberate live check.

For the offscreen missing-device regression, `HEYA_TEST_NO_MICROPHONE=1` in
`--demo` selects a synthetic absent input. It exercises the same helper, error,
dialog and retry path without creating a source/pipeline. The override is ignored
outside demo mode; normal recording always checks the selected real device.


Settings → Audio selects a microphone and speakers, recording volume and playback
volume. “System default” resolves a device when an operation starts; a saved
explicit selection stays selected while unplugged and is not silently replaced.
Open settings to discover devices initially, or use Refresh devices after changing
hardware. Discovery is a one-shot helper, not a background device monitor. Volume
and routing changes apply to the next recording/playback. Discovery does not capture sound, and an absent input prevents the recording pipeline from starting.
Playback settings apply to both received audio and review playback. Synthetic demo
input intentionally varies its amplitude so render checks keep exercising waveform
changes after warm-up.
