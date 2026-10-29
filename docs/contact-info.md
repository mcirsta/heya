# Contact info

The conversation’s top-right menu opens **Contact info** (or **Group info**).
The panel follows the supplied WhatsApp Web screenshots: centered picture, name
and phone number, Voice/Video/Search actions, a four-photo strip and separated
chat controls. It docks beside a wide conversation and covers the conversation
on smaller windows. Escape returns from shared items or closes the panel.
Clicking the chat-header picture or name/status area opens the same panel.
The phone number supports text selection and Ctrl+C (Command+C on macOS),
while remaining read-only.

Voice and video reuse the existing checked call flow. Search opens saved-history
search. Mute durations, unmute, archive/unarchive and lock use the existing
backend-confirmed chat actions, with progress and failure feedback inside the
panel. Locking keeps the existing unlock-on-phone policy. Merely opening the
panel never places a call, sends a message or marks media viewed.

**Media, links and docs** opens a browser with separate tabs. Counts describe
available saved history on this device; hidden, deleted, expired and locked
content is excluded. Each tab shows its latest 100 matching items and states
when older matches remain. Links currently use stored link-preview metadata;
plain text URLs without previews are not indexed by this browser. Photos open
the existing album viewer, videos use existing playback, documents use Save As,
and explicitly selected web links use the existing URL validator and launcher.
The full URL is kept independently of shortened display labels.

The panel does not invent About text, groups in common, privacy settings or a
verification state. Contact editing, starred messages, favorites, export,
block/report and security-code verification need their own complete data/action
paths before appearing here. A known nonzero disappearing-message timer is
shown read-only; the archive’s default zero is not presented as a confirmed
remote “Off” setting.

## State and work bounds

One latest-value demand uses the existing Tokio runtime and archive worker.
There is no dedicated thread, polling timer, schema migration or second media
download service. Closing/navigation replaces demand immediately; the UI also
matches the requested chat and request identity before accepting results.
Archive bursts coalesce behind a finite read, rather than accumulating blocked
database jobs. Closed panels do not query on archive changes, and a disappearing
archive/UI receiver ends the task.

Queries read metadata without original envelopes or media. Counts scan eligible
metadata on the archive worker; shared-item models are bounded to 100. The
existing photo service/cache owns decoding and textures: four profile previews
or at most sixteen nearby browser items share its total demand limit of 24.
Changing the range releases old preview demand. Publishing panel data does not
rebuild or replace the conversation/transcript. Backend media operations
revalidate eligibility when a clicked ID is outside the loaded chat page.

The supplied screenshots are visual references only. Application logic, icons
and component code are Heya’s own implementation.

## Reproduce validation

Synthetic tests cover aliases, names, phone-number privacy, eligibility, counts,
limits, unchanged unread/media data, late results, close during a blocked read,
failed reads, action routing and light/dark/narrow layout containment. No test
sends a real message or places a real call.

```sh
HEYA_DEMO_UNLINKED_ALBUM=1 scripts/ui-offscreen.sh --framebuffer /tmp/heya-contact.png --demo-chat 9 --size 1200x900 --light --click 1172x30 --click 1050x62
HEYA_BENCHMARK_CONTACT_INFO=1 HEYA_TEST_RENDERER=winit-skia-opengl scripts/ui-offscreen.sh --ui-benchmark
```

The focused benchmark uses synthetic SQLCipher data with 500 chats and 10,000
messages. It exercises the real contact-info/photo publication paths, tabs,
resizing and close/reopen cleanup. Timings exclude network, GPU completion and
compositor presentation; native macOS validation remains separate.

## Linux validation — 28 October 2026

336 application tests passed with media and 317 without it. Strict Clippy passed
for both configurations, along with formatting and shell checks. Native
light/wide, dark/narrow and shared-media captures were inspected after correcting
the header position and preview-strip containment. Synthetic close checks found
no idle media thread and exited in 22–47 ms.

Optimized OpenGL/Xvfb measurements used SQLCipher on the project filesystem.
The focused benchmark retained the same 100-row conversation model throughout;
maximum photo demand and snapshot residency were 12 items, under the shared
limit of 24. Results include worker/event-loop scheduling where indicated:

| Measurement | p95 |
| --- | --- |
| Command to observed contact data ready (17 samples) | 90.51 ms |
| Profile to four textured previews (10 samples) | 224.87 ms |
| Close to preview release (6 samples) | 16.79 ms |
| Command to render-complete callback (23 samples) | 122.61 ms |
| Photo UI publication during panel resize | 0.69 ms |

A paired ordinary benchmark with the panel closed measured 10,000-message scroll
render CPU p95 at 12.91 ms before and 12.49 ms after; resize p95 was 17.46 / 16.04 ms.
Unchanged snapshot publication remained 0.12 ms at p95 in both runs. Cold initial
rendering took roughly two seconds in both. The panel's resize phase still had
isolated render CPU spikes up to 95.41 ms, despite p95 of 12.97 ms. These Xvfb
runs reported unavailable DRI3 acceleration and Mesa fallback; they do not prove
smooth native GPU/compositor resizing or resolve the existing renderer spikes.

Baseline binary SHA-256: `2d245c1c349786d6a8898f68ff3a6098848ba4b2c35cbc50e8c4a7dd025d6a38`.
Contact-info binary SHA-256: `0f1bda2840354186ddd16930f69701a83ff1b97b18070912b5224d875007f235`.
Both use the same pinned dependencies. The paired run used the ordinary
`--ui-benchmark`, `HEYA_TEST_RENDERER=winit-skia-opengl` and
`MESA_SHADER_CACHE_DISABLE=true`; `HEYA_TEST_BINARY` selected the baseline.

The phone-selection/header-entry follow-up passed all 336 default-feature tests
and strict Clippy with and without media. A separate private-X11 check opened
the panel by clicking the header name and picture, dragged across the phone
number and verified its exact text through another clipboard process. Typing
could not alter it. The toolkit tests also exercise selection/copy through the
private test clipboard. Command+C uses Slint's existing macOS modifier mapping;
this Linux check does not establish native Apple clipboard compatibility.
