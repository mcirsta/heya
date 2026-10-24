# Direct video calling

The direct-chat camera button starts a two-way call; incoming video offers have
Answer and Decline controls. Audio and camera/codec preflight complete before
outgoing signaling. The camera remains closed while an incoming offer rings.
No camera, denied permission, missing H.264 codecs and stalled device work produce
visible errors, with cancellation available during the check. Each readiness
deadline covers discovery, opening, encoding and decoding the first test frame.

The video panel uses a dark remote stage, a small self preview, connection status,
camera and microphone controls, and a separate red hangup control. Its visual
reference is Meta's [WhatsApp Web calling announcement](https://about.fb.com/news/2026/07/whatsapp-web-calling-new-features/).
Turning the camera off stops capture and tells the SDK to stop only outbound
video. Incoming video and duplex audio continue. Re-enabling the camera waits
for real capture before resuming signaling. Failures end and release the call
rather than leaving the UI and backend in different states.

## Ownership and bounds

The existing pinned SDK public video source/sink API supplies signaling,
encryption, H.264 packetization, relay transport and keyframe requests. No new
SDK patch is required. `protocol_call_video` converts timestamped Annex B access
units and SDK camera events at the adapter boundary.

`call_video` owns a finite subprocess. Linux uses GStreamer with x264 (or
OpenH264), and macOS uses AVFoundation/VideoToolbox. Native camera and codec
objects never enter the Slint event loop. Camera output is 640×480 at 15 fps;
decoded images are bounded to 960 px in each dimension. Encoded access units are
limited to 2 MiB. Bounded queues discard old frames under pressure and request a
fresh keyframe after loss. The latest local/remote images use a separate coalesced
presentation stream, so video does not rebuild conversation models.

The camera helper feeds locally encoded frames into its decoder until an image
decodes before declaring readiness. macOS requests bounded encoder frame delay
where supported; Apple's [frame-delay documentation](https://developer.apple.com/documentation/videotoolbox/kvtcompressionpropertykey_maxframedelaycount)
describes the otherwise unlimited default. Camera-off acknowledgements follow
physical capture shutdown.
The parent monitors actual encoded progress while capture is enabled; heartbeats
only establish liveness while it is off. Hangup, disconnect, setup cancellation,
failure and app close stop/reap both media helpers and release the SDK bridge.
There is no video device worker at idle. The OS chooses the default camera;
camera selection, group calls and upgrading an existing voice call are separate
increments.

## Validation

Synthetic controller tests cover incoming/outgoing calls, two-way frames, camera
off/on, mute, hangup and closing during setup or an active call. UI tests exercise
the actual header and call controls, delayed signaling, readiness failures and
containment at narrow sizes. The ignored `native_camera_codec_loopback` test
uses a generated camera image with the real platform H.264 encoder/decoder,
including continued decoding while local capture is off. CI runs this on Linux
and Apple Silicon without an account, camera or microphone.

Use `HEYA_TEST_CALL_VIDEO_NATIVE=1` with the exact ignored codec test; use
`HEYA_TEST_CALL_VIDEO=1 HEYA_TEST_CALL_STATE=5 scripts/ui-offscreen.sh --framebuffer
/tmp/heya-video-call.png --size 1200x800` for a synthetic panel. The optimized
`HEYA_BENCHMARK_CALL_VIDEO=1 scripts/ui-offscreen.sh --ui-benchmark` case alternates
two prepared images at 15 fps alongside resize/background activity. It measures
application/rendering work, not camera, codec, compositor or network latency.

Automated tests never place a real call. They do not establish physical camera
compatibility, permission prompts or WhatsApp peer interoperability; those need
deliberate manual calls on each platform.

### Linux offscreen measurement, 2026-10-24

An optimized build with Skia/OpenGL in Xvfb measured ordinary 10,000-message
scrolling at 13.02 ms render CPU p95 (previous check: 13.36 ms). With prepared
video images updating at 15 fps, changing background snapshots measured 11.04 ms
p95; resizing the 10,000-message view measured 19.79 ms. The modal covers the
timeline, so its scrolling number is not a direct comparison with ordinary chat.
Existing cold rendering (~2 s) and first-resize (~0.4 s) outliers remain. Native
window-close checks took 22–44 ms and found no idle media threads. These are
application/rendering checks, not compositor, camera, codec or network latency.
