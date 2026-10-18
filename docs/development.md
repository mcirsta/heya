# Heya development preparation

The workspace contains a Cargo application with live text messaging, an encrypted archive and
an offline Slint demo. Run `cargo run --release --locked` to resume a saved session
or show the first-use connection screen, `cargo run --release --locked -- --offline`
to open saved chats without connecting, or `cargo run --locked -- --demo` for
synthetic chats. See the
[README](../README.md) for controls and the [roadmap](roadmap.md) for the remaining
M0 integration checks.

## Archive-to-UI optimization — 19 October 2026

Measured two baseline runs at `1dacd34` and two final runs of the changed build,
using the existing production-path synthetic benchmark: disposable SQLCipher
archive, 500 chats, 10,000 loaded messages, real messaging worker, coalesced UI
handoff and Slint render-completion callback. No compiler or test job ran during
measurements. The machine used a Xeon E5-2697A v4, CachyOS Linux 7.2.9, Rust
1.99.0, Slint 1.18.1 and Skia bindings 0.153.3. All native UI measurements used
private Xvfb/Mesa software OpenGL via `scripts/ui-offscreen.sh`, without touching
the working desktop or opening an account.

Build and measure with:

```sh
cargo test --release --locked --config 'profile.release.build-override.debug-assertions=true'
HEYA_BENCHMARK_ARCHIVE=1 HEYA_TEST_RENDERER=winit-skia-opengl scripts/ui-offscreen.sh --ui-benchmark
```

The build override emits Slint element metadata needed by synthetic layout
checks; it does not enable application runtime debug assertions. Both baseline
and final executables used this optimized configuration.

| Measurement, milliseconds unless noted | Baseline runs 1 / 2 | Final runs 1 / 2 |
| --- | ---: | ---: |
| Incoming burst, 10,000 loaded: view/projection p95 | 87.50 / 103.59 | 17.94 / 18.61 |
| Incoming burst: ingest start → render completion p95 | 175.07 / 197.36 | 68.07 / 70.15 |
| Incoming burst: ingest start → render completion maximum | 184.02 / 202.29 | 70.69 / 74.07 |
| Incoming burst: UI application p95 | 18.59 / 22.61 | 9.46 / 10.34 |
| Unrelated burst, 10,000 loaded: view/projection p95 | 85.49 / 82.63 | 7.46 / 7.65 |
| Unrelated burst: ingest start → render completion p95 | 155.80 / 155.76 | 50.12 / 50.18 |
| Unrelated burst: ingest start → render completion maximum | 166.63 / 164.04 | 50.90 / 50.99 |
| Navigation: command enqueue → render completion p95 | 54.60 / 50.91 | 53.96 / 53.71 |
| Scrolling render CPU p95 | 10.88 / 10.74 | 10.75 / 10.85 |
| 99 actual paging commands, total seconds | 4.71 / 5.15 | 4.58 / 4.86 |

Bursts contain twenty messages per ingest. Incoming phases issued 720 / 620
messages and applied/rendered 36 / 31 coalesced generations before, versus
920 / 920 messages and 46 / 46 generations after. Unrelated phases issued
720 / 700 messages with 36 / 35 generations before, versus 940 / 940 with
47 / 47 after. Latencies describe observed snapshot generations, not individual
messages; the producer waits 50 ms after each ingest, so reduced worker/lock
cost also allows more ingests within its fixed three-second phase.

The archive now retains one immutable projection of the loaded conversation.
A single in-memory SQLite TEMP marker tracks whether relevant writes invalidate
it or strictly later visible inserts can extend it. Transaction rollback restores
that marker automatically. Shared names, aliases, identity and control changes
invalidate conservatively; external commits are detected on the next view using
`PRAGMA data_version`. Drafts, sidebar summaries, replies, permissions and history
state are refreshed independently. The cache also retains equal payloads after a
conservative invalidation; publication no longer performs a second value-comparison
step that could leave the cache and UI with different payload identities.
Repeated unchanged push names skip the contact write instead of invalidating the
projection on every live envelope.

Strict appends query/decode only the new rows and clone retained domain messages,
reusing their decoded content. Transcript construction copies the retained text
and formats only new/dropped boundary messages, preserving Unicode and embedded
blank lines. A reference to an existing reaction/control event, an edit, deletion,
delivery change, out-of-order insert, page-size change or changed context uses the
full projection.
No general diff engine, per-message revision map, dependency, persistent schema
migration, extra worker or polling loop was added. Deselecting releases the cache.
The full query reads the visible timeline index backwards and reverses its row
vector instead of materializing `SELECT *` and sorting again.

Incoming phases recorded 46 / 46 append projections. Unrelated phases recorded
47 / 47 cache reuses and zero timeline/transcript rebuilds. Append projection p95
was 7.18 / 7.45 ms; transcript formatting/copying p95 was
1.21 / 1.16 ms, and unchanged timeline lookup p95 was 0.05 ms in both runs.
An intermediate cache-only run still spent 83.30 ms p95 projecting incoming
updates, including 7.68 ms formatting their transcripts; this justified the
narrow append path rather than stopping at publication-time pointer reuse.

This remains an array/string representation: appends still copy retained message
strings and transcript bytes, so they are not constant-time. Paging and navigation
show no established improvement. Cold offscreen startup still included a roughly
1.7-second first render; the first incoming phase retained cold maxima of
308 / 370 ms (baseline 306 / 319 ms). These are not claimed fixed. The measurements
exclude compositor presentation, hardware GPU completion, actual input-device
latency and phone/network behavior; two runs are a local comparison, not a
statistical guarantee or a ZapFast comparison.

The optimized suite passed 210 application tests plus the encrypted-storage
integration test, with three intentional opt-in/helper skips. Nine new tests
cover reuse with fresh sidebar/drafts/replies/read flags, text/attachment/delivery
changes, names/quotes/reactions, paging/expiry, rollback/external commits, cache
release, index plans without temporary sorts, rolling append windows compared
with fresh projections, ambiguous wire keys and pending-control dependencies.
Historical-schema upgrade tests still pass; temporary cache state adds nothing
to saved archives. Native PDF/portal tests ran outside Codex's sandbox using
synthetic inputs and private test services, with no visible desktop checks.
Strict release all-target Clippy passed with and without GStreamer support; only
the existing vendored Parley deprecation warning remains. Formatting and diff
whitespace checks passed. The installed Linux executable was replaced atomically
for the next normal launch, and generated build/benchmark outputs were removed.

## Linux validation after the macOS port and master-key change — 18 October 2026

Fast-forwarded the Linux checkout from `8f26304` to `07b2d78` (11 commits),
version `0.1.0-alpha.1`. Validation used the installed Arch Linux Rust/Cargo
1.99.0 toolchain without installing packages or changing toolchains. Linux
retains GStreamer, Secret Service, desktop portals, sandboxed Poppler previews
and the Skia Vulkan/OpenGL feature selection. Apple framework adapters and
emoji positioning remain scoped to macOS.

Formatting, strict all-target Clippy and tests passed with both feature
configurations: 193 application tests with default media support and 176
without it, plus the encrypted-storage integration test in each configuration.
The ordinary suites each left two opt-in/helper entries ignored. The native
Secret Service test was then run explicitly with a disposable credential and
temporary encrypted database; fresh-handle reopen and cleanup passed. The
synthetic master-key test independently verifies one vault read on reopen,
distinct durable session/archive keys, and rejection of the other database's
key. These tests do not establish a new live-account pairing or microphone
compatibility. Only the existing upstream Parley deprecation warning remains.

The optimized build passed and resolves its native GStreamer libraries. All
native automated UI work used `scripts/ui-offscreen.sh` on private Xvfb displays,
with no compiler running during measurements. Startup/demo close cases exited
in 40–75 ms without idle media threads. Mouse-wheel input, light color emoji,
dark narrow photo layouts and sharp synthetic PDF cards passed smoke/visual
checks. A paired 10,000-photo workload measured scroll render CPU p95 of
10.15 ms before and 10.24 ms after, with 187 frames and zero remeasured rows in
each scrolling phase. Layout p95 was 0.14 ms in both. This small comparison
shows similar behavior; it is not a statistical equivalence or hardware-GPU
claim.

The application-path archive benchmark completed all eight phases and 99 real
page commands. At 10,000 loaded messages, incoming bursts measured 170.85 ms
p95 from ingest start to render completion (680 messages issued, 34 coalesced
generations applied/rendered); navigation measured 49.79 ms p95 from enqueue
to render completion. The private Mesa/OpenGL display does not measure physical
input, phone/network latency, GPU completion or compositor presentation.

The macOS emoji patch correctly reproduces the vendored Parley files, but its
recorded checksum was stale. `prep/upstreams.json` now records the actual patch
hash and its macOS scope; no application or vendored runtime code was changed
during this validation. Build inputs and local evidence are recorded under
ignored `.prep/validation/linux-sync-20261018/`.

The new storage implementation uses `master-v1` and the `default-v2` profile;
it deliberately does not migrate the former profile. QR pairing is required
again. The former archive and keys are retained. The verified Linux build was
installed through the regular desktop installer and launched normally, with
matching built/installed SHA-256:
`05727b2b97b3e8278249645bc313f3ca97d10275eb6838a096dfb0d88faa4caf`.
macOS execution and signing were not retested on this Linux host. M0/M2
acceptance gates remain open.

## SVG branding and Linux desktop icon — 15 October 2026

The owner selected the lowercase-h speech bubble. Its source is
`ui/assets/heya.svg`, with a transparent background, a fixed teal fill and a
white h drawn as paths. The window icon and connection screen share that SVG.
The application sets `org.heya.Heya` after Slint backend initialization and
before native window creation, matching the desktop entry and icon name.

`python3 scripts/install-desktop.py` installs the current optimized binary and
launcher for the user. It preserves the scalable SVG and generates eight PNG
fallbacks from the same artwork before modifying the installation. See
[branding](branding.md) for the paths and rebuild/update procedure.

Validation: the optimized build, formatting, all-target Clippy with warnings
denied, and the media-disabled build check passed. The ordinary suite passed
91 unit/UI tests and one storage integration test; the opt-in desktop-keyring
test stayed skipped. A complete installation into temporary directories passed,
including the executable, validated desktop entry, matching SVG and all eight
PNG sizes. The final optimized binary ran through `scripts/ui-offscreen.sh` on
a private Xvfb display: its WM_CLASS was `org.heya.Heya`, and its published
64×64 native icon contained transparent pixels, the exact teal fill and the
white h. The synthetic framebuffer capture succeeded. These checks establish
X11 window metadata and icon publication, not compositor/taskbar rendering on
the owner's Wayland desktop. No live account or visible desktop check ran.

## Received photos — 15 October 2026

Ordinary received JPEG/PNG images now retain their caption, dimensions and embedded
thumbnail during live/history decoding. Content decoder revision 2 upgrades older
saved envelopes locally. Schema 6 adds originals and derived previews to the same
SQLCipher archive; no plaintext media files are created. Photo rows are removed
on mutation, deletion or expiration, and guarded writes reject obsolete requests.
An exact duplicate merged across PN/LID identities can retain its verified cache,
including when another download was in flight. A late failure cannot demote that
verified result.

`protocol_photos` uses the pinned SDK's `download_to_writer` API and existing
streaming Ureq transport. Only successfully authenticated downloads enter the
archive; the SDK checks the MAC and supplied plaintext/encrypted SHA hashes.
Heya supplies a size-limited in-memory writer and rejects missing keys/hashes,
static unencrypted URL routes and declared oversize files before download. The
20 MiB limit also applies while writing when no length was supplied. Downloads
have a 45-second deadline; cancellation revokes the writer and holds its permit
until any SDK blocking I/O unwinds. Originals stay private to that attempt until
verification completes. Failures use fixed statuses without logging media URLs,
keys, payloads or captions. No upstream patch or new dependency is needed.

One download and one blocking decode run at a time. Demand is capped at 24 nearby
messages plus the one explicitly opened viewer. Switching away cancels unused
work, and opening another viewer takes priority over prefetch. Failed attempts
do not loop automatically; explicit retry is coalesced while an attempt is active.
The auto-download preference defaults on, persists with existing settings, and
can be disabled independently of explicit photo activation.

Decoding validates JPEG/PNG format, 16,384px per-axis and 32-megapixel limits, uses
the decoder's 128 MiB allocation limit, and applies EXIF orientation. Small images
retain their native dimensions: the image crate's thumbnail helper otherwise
upscales them to the requested limit. A synthetic JPEG with a 90° EXIF orientation
tests that both preview and viewer apply orientation once without upscaling. An encrypted
640px derived preview avoids repeatedly decoding originals during scrolling.
Decoded images are immutable and reused across unrelated archive updates; leaving
the demand window releases them. A viewer gets a separate decode bounded to 2048px
only while open. Closing it immediately clears its UI image and demand. These are
working-set/transfer bounds, not a disk-cache capacity policy; disk eviction,
media re-upload requests for expired CDN references, copy/save and sending remain
future work.

Each inline row adds one clipped native Image and a compact status/click target.
A 44px disc shows download/retry or a rotating 22px glyph while loading; the
loading text banner has been removed. Status text remains accessible and is
available in the opened viewer.
There are no per-row timers, scroll containers, viewers or external document
engines. Captionless photos overlay the timestamp; other captions remain native
selectable text. Declared dimensions reserve the frame before any thumbnail or
full download arrives. Missing or incorrect dimensions use a bounded contain
frame instead of shifting the scroll anchor after decoding.

Use `--demo --demo-chat 9` for procedural landscape, portrait and panorama photos.
`HEYA_BENCHMARK_PHOTOS=1 scripts/ui-offscreen.sh --ui-benchmark` exercises the same
worker-to-UI image handoff with a synthetic 10,000-photo conversation. This original
fixture reuses three decoded images; it does not measure archive reads/decodes.
The private
Xvfb wrapper also accepts `--click XxY` with framebuffer capture, solely for
clicking synthetic controls before capture; it never operates desktop windows.

Validation passed 73 unit/UI tests and one integration test; the opt-in real
keyring test was skipped. Formatting and all-target Clippy with warnings denied
passed. A final focused JPEG check passed after the lint-only test adjustment.
These are synthetic protocol/storage/UI checks, not validation of a real-account
media download or desktop clipboard integration.

Offscreen framebuffer checks passed at 1200×1100 light and 520×1100 dark,
including portrait, landscape, panorama and captionless timestamp containment.
A private-Xvfb click also opened and rendered the larger viewer. Toolkit checks
cover fixed geometry before/after image arrival, retry/auto-download controls,
zoom/close/Escape, deletion while viewing, and fewer than 30 instantiated rows
through seeks/resizes in a 10,000-photo conversation. Storage/worker checks cover
schema-5 upgrades, encrypted reopen, exact duplicate merge during a download,
mutation/expiry/obsolete-result guards, bounded decoding and residency, pixel reuse,
offline/manual download, cancellation and failure retry. Functional wait deadlines
for thumbnail/photo worker tests allow 15 seconds for unoptimized decoding under
parallel test load; they are not performance thresholds.

Release measurements ran sequentially after compilation, with wallpaper and
sidebar avatars enabled, using only synthetic data. Two matched Skia CPU/Xvfb
text runs per binary (second pair in reversed order) and the final photo run produced:

| Layout p95 | Previous text build | Current text build | 10,000-photo workload |
| --- | --- | --- | --- |
| 10,000-message scrolling | 0.24–0.25ms | 0.26–0.30ms | 0.19ms |
| 10,000-message resizing | 14.17–17.44ms | 16.73–16.81ms | 2.93ms |

Text resize timings overlap the baseline's variation; the observed text scrolling
increase is 0.01–0.06ms. This is a small sample, not a statistical equivalence
claim. The photo workload reported 3–4 instantiated rows and loaded images at
phase boundaries. Its larger row heights mean fewer visible rows, so the numbers
do not imply that adding photos speeds up an otherwise identical conversation.
Skia CPU frame callbacks are unavailable; printed zero frame times are not results.

Separate offscreen Skia/OpenGL runs exercised texture rendering. GLX reports Mesa
llvmpipe (software, not accelerated). For 10,000-message scrolling/resizing,
render CPU p95 was 40.31/52.62ms for text. Photo runs before/after the small-image
decode correction ranged from 18.86–27.67ms for scrolling and 27.42–32.60ms for
resizing, retaining 3–5 rows/images at phase boundaries. The synthetic renderer
fixture itself is unchanged by that decode correction; variation is not an
attributable speedup. These
software-rendered results are useful regression/smoke checks, not evidence of
native Vulkan throughput, compositor animations, 60fps, or live network behavior.
No native desktop benchmark, account screenshot or automatic message send ran.
Evidence, source/binary hashes and parsed results are in ignored
`.prep/validation/received-photos-20261015/`.

## Conversation dates and spacing — 15 October 2026

The timeline now uses the pinned ZapFast reference's local-day chips and sender
run spacing. Calendar dates are computed on the archive worker; visible labels
are cached in the existing virtual row cache. Chips are conditional raised
surfaces with a single Text, not calendar widgets. The shared measuring row uses
a font-height probe independent of lazy chip creation, so first render and
recycled rows have the same geometry. Unknown timestamps display no invented
calendar date. Time-only footers retain delivery state; copied transcripts retain
full dates. No archive migration, dependency or protocol-library patch is needed.

One timer per open timeline checks the current local date once a minute and
changes labels only after a day change. Rollover leaves heights and scroll
positions unchanged. Loading history, editing timestamps and deleting messages
invalidate adjacent day/run boundaries. The anchor also records the measured
space above the bubble, so removing a page-boundary date chip leaves the visible
text stationary. There is no per-message timer or full-history widget list.

The synthetic conversation `--demo --demo-chat 8` shows older, yesterday and today
messages plus consecutive messages and sender changes. Timeline lab and the
benchmark now include a day boundary every 20 messages. The worst-case toolkit
regression has 10,000 rich messages, each on a separate day, and retains fewer
than 30 nearby rows across seeks/resizes.

Validation passed 66 unit/UI tests plus one integration test; the opt-in real
keyring check was skipped. A final focused toolkit run also passed after adding
the history-prepend text-position assertion. Tests include unknown/future dates,
year rollover, local vs UTC dates and 23/25-hour day examples, archive-to-view and
transcript formatting, day-boundary deletion/prepending, midnight label rollover
without remeasurement, and native geometry in both themes at 520/620/1200/1920px.
Formatting, shell syntax and all-target Clippy with warnings denied passed.

The optimized release build passed offscreen framebuffer inspection at 1200×1000
light and 520×1100 dark, using only synthetic messages. One Skia CPU/Xvfb run
before and one after used the same 10,000-message preview/action workload, with
wallpaper and sidebar avatars enabled; the new fixture adds local-day boundaries
every 20 messages. Results are layout p95, not GPU or compositor frame timings:

| Phase | Previous build | Date separators |
| --- | --- | --- |
| 10,000-message scrolling | 0.29ms | 0.28ms |
| 10,000-message resizing | 6.20ms | 5.75ms |
| 100-message scrolling | 1.72ms | 1.67ms |

These short runs show no layout regression in the exercised workload. They do
not establish a speedup: chips change row heights/visible counts, most scrolling
reuses cached measurements, and CPU-renderer frame counters are unavailable.
The measured row counts for 10,000-message resizing were 380 before and 306 after.
The native geometry stress test separately exercises a separator on every row.
Evidence and source/binary hashes are in ignored
`.prep/validation/timeline-dates-20261015/`. No live messages or links were
activated during validation.

## Native message cards — 15 October 2026

Link previews now retain the sender-supplied title, description, URL and embedded
thumbnail. Business templates retain a separate display body and up to eight
native action rows; the full transcript still contains action values. HTTP(S)
actions launch the system browser only on activation, and copy-code actions use
Slint's existing clipboard path. Other action types remain explicitly unavailable
on desktop. URLs are validated separately from captions and again before launch;
browser invocation is serialized on a blocking worker, without URL logging or
shell commands. The direct `webbrowser` dependency is pinned to 1.2.4, already
present transitively in the lockfile. No protocol dependency changes were needed.

Schema 5 adds encrypted display metadata and embedded thumbnails. Startup repair
recovers eligible older rows from their saved envelopes, in transactions of 100,
with a completion marker and restart after failure. It preserves drafts, unread
state and message identity. Edits clear stale cards/actions; deletion and expiry
clear the materialized content and image bytes. The existing mutation protections
prevent recovery or alias merging from restoring deleted/expired content.

The timeline still instantiates only visible/nearby rows. Thumbnail demand is
capped at 32, encoded input at 256 KiB each, and decoded output at 128×128 RGBA
(64 KiB each, at most 2 MiB per retained pixel set before UI/renderer copies).
The existing decoder also bounds dimensions and allocation. Database reads and
decoding run off the UI thread; corrupt inputs keep the card's reserved space.
Unchanged archive events reuse pixels without decoding or re-uploading, and
leaving the demand set evicts them. There are no automatic website requests.

Slint's lazily instantiated action repeater initially made hidden measurement
depend on whether its children had already been created. The 10,000-card
resize regression caught overlapping rows. Action height now derives directly
from the action count and a shared 44px metric, with two-line labels bounded
inside each hit row. Preview height is reserved at 88px to fit the title,
two-line description and domain independently of image arrival. Both the probe
and displayed rows use those same components. Empty Slint Images compare unequal
even to themselves, so timeline reconciliation handles that case explicitly to
avoid needlessly invalidating plain-text rows.

The synthetic Message cards conversation can be opened with `--demo --demo-chat 7`.
The offscreen runner accepts that selector for screenshots/framebuffer captures.
`HEYA_BENCHMARK_MESSAGE_CARDS=1 scripts/ui-offscreen.sh --ui-benchmark` exercises
alternating previews/business actions across 10,000 messages with bounded
synthetic thumbnail textures. It does not exercise network/real account data or
thumbnail decoding; separate worker tests cover decoding and eviction.

Validation: the final full suite passed 63 unit/UI tests plus one integration
test; the opt-in real-keyring test was skipped. Formatting, shell syntax and
all-target Clippy with warnings denied passed. Tests cover protobuf extraction,
live/history consistency, encrypted schema-4 upgrade/reopen, interrupted repair,
alias merges, edits/revokes/expiry, corrupt/oversized images, 32-image residency,
eviction and unchanged-pixel reuse. Native toolkit tests check click routing,
mocked clipboard copy/paste with Unicode, label/card containment and no scroll
shift on thumbnail arrival/removal. The 10,000-card test resizes and seeks while
keeping fewer than 30 instantiated timeline rows and matching measured/drawn
heights. Browser launching and real desktop clipboard interoperability are not
established by these headless tests.

Final offscreen framebuffer captures were inspected at 1200×800 light and
620×600 dark. A 1200×800 OpenGL capture also showed the cached thumbnail and
wallpaper correctly; Xvfb's missing DRI3 means this was not a hardware-accelerated
driver test. Earlier captures caught the divider-position and description-line
height issues corrected in the final build. All captures contain synthetic data.
Artifacts, raw benchmark logs and source/binary hashes are retained in the
ignored `.prep/validation/message-cards-20261015/` directory.

Four sequential release runs used Skia CPU under Xvfb, in plain/cards/cards/plain
order, retaining wallpaper and sidebar avatars. Each ran for about 30.3–30.7s
including startup and Xvfb. Results across the two runs of each workload:

| Measurement | Plain-text workload | Preview/action workload |
| --- | --- | --- |
| 10,000-message scrolling layout p95 | 0.25–1.84ms | 0.26–0.27ms |
| 10,000-message resizing layout p95 | 16.77–17.17ms | 6.43–8.36ms |
| Unchanged snapshot layout p95 | 0.06–0.07ms | 0.07ms |
| Whole runner user + system CPU | 17.69–18.51s | 17.82–18.22s |

These workloads have different row heights, so the cards leave fewer rows visible;
the resize result does not establish that a card is cheaper than a text row.
Most scrolling measurements reuse cached heights, while the resize phase forces
remeasurement. The aggregate CPU ranges overlap. Skia CPU does not provide this
benchmark's render/frame callbacks (printed zeros are unavailable counters).
Software rendering still produces substantial timer lateness; these results do
not establish compositor smoothness, GPU frame time or a comparison with ZapFast.

The normal release instance was reopened after these checks and reached Connected.
No live message, business action or browser link was activated by validation;
the owner's two specific messages still need visual confirmation in their chats.

## Wallpaper coverage and resize cost — 15 October 2026

An isolated synthetic screenshot reproduced the reported centred single-tile
background. The Image had tiling enabled but lacked an explicit drawing area.
The conversation wallpaper and its single repeating Image now have explicit
bounds, a window-anchored origin and panel clipping. This matches the pinned
ZapFast's 374×666 logical-pixel repeat instead of stretching the artwork.

Inspection of Slint 1.18.1's Skia `cached_image.rs::as_skia_image` found that the
SVG branch reads the target-size binding and rerasterizes after resize
invalidation. The embedded-raster branch has no target-size dependency. Heya
therefore bundles a fixed PNG derived from the source SVG and decodes it once
on the existing startup blocking worker. Ready pixels are supplied before the
window is shown; no decoding is scheduled by scrolling or resizing. Skia's
`draw_image_impl` caches the tinted image and draws its repeat shader across the
clipped area. Theme/scale-factor changes can refresh renderer resources; normal
resizing does not create a new source image. No new dependency, worker, tile
grid, full-window cached wallpaper or cache-size setting was added.

The asset is 61,091 bytes packaged, with 996,336 bytes of RGBA pixels before
renderer copies. Its dimensions remain fixed as the window grows. Attribution
and the reproducible rasterization command are in the third-party notices.

The headless Slint regression checks full panel coverage, containment, window
anchoring and one Image across light/dark, demo/live layouts, banners,
expanded/collapsed sidebars, and repeated sizes from 520×480 to 3756×2033.
It also checks the runtime tile dimensions and hiding doodles. The full suite
passed 59 unit/UI tests plus one integration test; the opt-in real-keyring test
was skipped. All-target Clippy with warnings denied passed.

The optimized build passed. Isolated captures were inspected at 1920×1080 and
3756×2033 in light mode, 800×600 in dark mode, and 1200×800 at 2× scaling.
A 520×480 capture checks the narrow sidebar; the headless geometry regression
explicitly opens the narrow conversation. Pixel comparisons on the wide capture
confirmed exact 374px horizontal and 666px vertical repetition, including near
the panel edges and bottom. At 2× scale the horizontal repeat is 748 physical
pixels. No live account contents or desktop windows were captured.

Four sequential release benchmarks ran through `scripts/ui-offscreen.sh` with
wallpaper on/off/off/on, retaining the existing synthetic 500-chat, 66-avatar,
100/10,000-message workload. Shell user+system time for the entire runner was
18.167/16.092/15.943/18.195 seconds respectively (each about 30.6s elapsed,
including startup and Xvfb). Wallpaper-on runs consumed about 13.5% more aggregate
CPU time; this is not an isolated per-frame rendering cost. Scroll-10,000 timer
lateness p95 was 15.41–18.61ms on versus 2.46–2.65ms off. The software rasterizer
still pays for filling/blending the background; this check does not establish
zero overhead or desktop smoothness. Skia CPU render/frame callback counters are
unavailable, so their printed zeros are not measurements.

An additional `winit-skia` check could not initialize Vulkan on Xvfb because DRI3
is absent and fell back to OpenGL without acceleration. Slint's `take_snapshot`
then omitted the cached tinted tile: it re-renders through a CPU canvas while
retaining the graphics image cache. The actual OpenGL window was correct.
The runner's new `--framebuffer` mode captures that synthetic window directly,
under the same isolated display and private application directories, and stops
only its disposable demo process. Its 1200×800 capture showed the artwork and
passed an exact 374px repeat comparison. Normal Wayland/Vulkan performance and
compositor animations remain unmeasured by these offscreen checks.
The corrected normal build reopened the saved session and reached Connected;
no live messages were sent by the validation workflow.

## Business-template text — 14 October 2026

A content-free, read-only diagnosis of the reported encrypted envelopes found
`TemplateMessage.format = InteractiveMessageTemplate`, with a native-flow body
and URL actions. Heya's previous `MessageExt::text_content()` mapping only
extracted ordinary and extended text. The pinned SDK already decodes these
templates; it needs no patch or additional dependency.

`protocol_business.rs` extracts readable template/interactive text, hydrated
variants, and known URL/call/copy display values. Internal identifiers and flow
payloads stay out of the interface. Line breaks and full message text are retained;
sidebar summaries use the existing bounded single-line rule. Malformed actions
do not hide the body. Actions are selectable text, not clickable buttons or
automated replies. Carousel cards, unhydrated templates and other unavailable
actions direct the user to the phone. Linked-device masking and view-once
boundaries are preserved.

The comparison used the unchanged ZapFast pin's
`backend/worker/interactive.rs::{parse,Text::interactive,Text::native_button}` and
`ui/conversation.rs::interactive_buttons`. ZapFast separates interactive bodies
from action buttons, filters executable URL schemes and supports more reply/card
behavior. Its alternate `title` action label informed a compatibility case here;
Heya's extraction and repair implementation remain independent.

Startup repairs eligible unsupported placeholders in encrypted transactions of
100 candidates before publishing the archive, on the existing blocking worker.
Only materialized text changes. A revision marker avoids rescanning on later
launches; interrupted repair retries safely. Edited/deleted/expired/invisible
rows are excluded, corrupt envelopes remain untouched, and one envelope is
decoded at a time. No schema change or new worker is required.

Synthetic regression checks cover the actual envelope shape, alternate hydrated
forms, long multiline Unicode, malformed/oversized actions, fallback labels,
view-once/masked content, live/history consistency, and a 205-message repair with
failure in its second transaction followed by reopen/retry. They check retained
drafts, unread state, raw envelopes, identities and mutation protections.
Validation: 59 unit/UI tests and one integration test passed without desktop
windows; the opt-in desktop-keyring test remained skipped. Formatting and
all-target Clippy with warnings denied passed, and the five focused regression
tests passed again after the final cleanup. A read-only check against three
reported envelopes confirmed that the decoder preserved their bodies and
supplied links, emitting only type/presence/result flags. No private content was
logged, no messages were sent, and the temporary diagnostic was removed. The
optimized build passed. The updated app reopened the existing archive, completed
the repair step and reached Connected without an archive error. The owner's
display confirmation remains pending.

## Incoming unread-state correction — 14 October 2026

The owner confirmed that pictures load, then reported a badge of 17 for a chat
with zero unread messages on the phone. Heya counted new incoming envelopes but
ignored `ReadSelf`/`PlayedSelf`, `MarkChatAsReadUpdate` and history unread totals.
Only selecting a conversation cleared its local count. This was an application
integration gap; no upstream patch, renderer change or new dependency is needed.

Schema 4 preserves the archive and drafts, migrates the existing counts, and adds
per-message unread/seen evidence, read positions, early-receipt IDs and manual
mark-unread state. Receipt positions come from message identities; inclusive
app-state ranges come from message timestamps. Phone reads can clear the covered
messages while preserving later arrivals. History totals cannot resurrect explicit
reads, and PN/LID merging deduplicates both messages and read evidence. Missing
receipt IDs remain encrypted until their messages arrive. A manual unread mark
with no messages displays a dot, including in the collapsed sidebar and filter.
Read metadata for absent chats stays out of the sidebar.

`protocol_sync.rs` extends the existing coalesced contact worker to read-state
events. The first upgraded connection, a relink or an unclean session requests
read-state (`RegularLow`) and contact snapshots using the pinned SDK. Incomplete
sync retains existing state and retries; it does not guess that every chat is
read. Missing message ranges likewise are not replaced with receipt wall time.
Shutdown leaves recovery dirty if receipt work was queued or failed. The existing
local clear-on-selection behavior remains; outgoing read receipts/read-state
updates are still pending and this change does not claim bidirectional parity.

Synthetic checks cover 17-to-zero correction, delayed/early/duplicate receipts,
newer messages within the same second, history overlap/partial totals, monotonic
read evidence, manual unread ordering, PN/LID and group deduplication, schema-3
upgrade/reopen, retained drafts, failed transaction rollback and 2,048 coalesced
read updates without phantom chats. The production SDK event mapper is exercised
with a synthetic own-device receipt; history tests exercise supplied unread counts.
`cargo test --locked --offline` passes 54 unit/UI tests plus one integration test;
the desktop-keyring test remains opt-in. All-target Clippy passes with warnings
denied. The optimized build passed. A native synthetic run completed with zero
frames for unchanged updates; frame pacing still misses the broader target
(scroll-10,000 p95 36.55 ms, resize-10,000 p95 66.69 ms). This single run does not
establish a performance improvement. The updated build reopened the existing
encrypted archive offline without an archive error. Live correction of the
owner's badge awaits connection and confirmation.

The owner requested that tests stop interrupting their desktop. All future
automated screenshots and UI benchmark runs use `scripts/ui-offscreen.sh` on
Xvfb, with the display/Wayland entry points removed, private temporary runtime,
config/data/cache directories, and synthetic data. The runner defaults to Skia's
CPU rasterizer to retain Skia's clipping, emoji and effects. Slint's separate
software renderer was unsuitable for this visual check because it omitted round
image clipping. The virtual-display run is not a hardware/compositor performance
measurement: Skia CPU does not provide the benchmark's render callbacks (zero
frame counters are unavailable measurements), and Xvfb has no window manager for
maximize/restore. Both the 1200×800 screenshot and 27-second synthetic run passed
on the isolated display; the screenshot retains circular avatars and shadows.
Visible native checks require prior agreement; normal application
reopening remains separate from testing.

## Avatar thumbnails — 14 October 2026

Contact/group pictures, the account avatar, the conversation header and the
collapsed rail now share circular thumbnail rendering. The reference inspection
covered ZapFast's `ui/widgets.rs::paint_avatar`, `account.rs::avatar` and
`backend/worker.rs::fetch_avatar`/`PictureUpdate`; Heya's worker, storage and
presentation are implemented independently. Existing avatar sizes are unchanged.
The pinned whatsapp-rust public picture API is sufficient; no SDK patch or
renderer change was made. Image, ureq and url become direct dependencies at
versions already present in Cargo.lock; no system packages are needed.

The encrypted archive migrates to schema 3, preserving drafts and existing
records. Fetches follow the visible range plus selected chat/account, reuse
positive/absent results, and back off on transient errors and rate limits.
Image decoding and database work stay off the event loop. A generation check
prevents late results from undoing picture removals or alias merges. Cached
thumbnails work offline without opening plaintext files. Full-size viewing,
channel/community-specific picture routes and group message-sender photos
remain outside this slice.

Synthetic validation covers JPEG/PNG bounds and center cropping, corrupt input,
offline startup, deduplication of self/account requests, positive/negative cache
reuse, cancellation during fetch, rate-limit backoff, retained pixels on transient
failure, schema-2 migration/reopen, PN/LID merges, stale removals, write rollback,
SDK picture-notification delivery, UI identity through reordering/filtering,
bounded demand, photo removal and stable pixel identity across unrelated updates.
The default suite passes 46 unit/UI tests and one integration test; the existing
desktop-keyring test remains opt-in. Clippy passes with warnings denied.
Native renderer and live-account avatar checks are recorded separately below.

The synthetic 1200×800 native capture confirms circular pictures and the existing
account/list/header sizes. The benchmark now includes 66 synthetic thumbnails;
`HEYA_BENCHMARK_NO_AVATARS=1` disables them only in `--ui-benchmark` for comparison.
The first comparison exposed repeated redraws from assigning Slint's empty Image
(which does not compare equal to itself). Presentation now skips unchanged empty
images as well as unchanged real buffers. The final release build passed.

The final sequential native pair (pictures off, then on) is recorded in
[`prep/avatar-thumbnails-2026-10-14.json`](../prep/avatar-thumbnails-2026-10-14.json),
including raw logs, source hashes and the release-binary hash. Diagnostics confirm
Skia's WGPU surface and hardware RADV on the RX 550, with Wayland environment and
no renderer override. No compilation ran alongside the measurements.

| Metric | Pictures off | 66 thumbnails |
| --- | ---: | ---: |
| Frames during 3 seconds of unchanged updates | 0 | 0 |
| Changing snapshot application p95 | 0.59 ms | 0.45 ms |
| Scroll 100 messages: frame interval p95 | 82.34 ms | 77.50 ms |
| Scroll 10,000 loaded messages: frame interval p95 | 68.91 ms | 68.13 ms |
| Resize 10,000 loaded messages: frame interval p95 | 110.31 ms | 106.15 ms |

This pair verifies the empty-image redraw fix but does not establish a statistically
reliable avatar performance difference. The absolute frame pacing is substantially
worse than the earlier backend comparison, even with pictures disabled. Its cause
is unresolved; the responsiveness acceptance gate remains open. These synthetic
runs do not measure live picture fetching or end-to-end account behavior.
The updated build subsequently opened the owner's existing encrypted archive in
offline mode without an archive error. Live profile-picture retrieval still
awaits confirmation after the owner connects; no messages were sent by validation.

## Self-chat label correction — 14 October 2026

The owner confirmed that phonebook synchronization now supplies the saved contact
name, then pointed out the remaining self-chat difference. ZapFast's pinned
`app.rs::self_title` uses the account profile name with `(You)`, bypassing the
local phonebook alias. Heya now applies that rule using its bound account identity
and separately persisted profile name. It does not rewrite the phonebook entry.
`SelfPushNameUpdated` is handled without waiting for a new text message. Avatar
initials skip punctuation and use the first/last word, including the `You` suffix.

Regression coverage includes different profile/phonebook/history names, PN/LID
alias resolution, sidebar/header/search agreement, profile-change events,
unchanged-value invalidation, blank names, another account's rejection, unchanged
other-contact names and offline reopen. No schema or renderer change is needed.
The full synthetic suite passed 39 unit/UI tests and one integration test; the
desktop-keyring test remains opt-in. The self-chat regression passed again after
the final iterator cleanup. Formatting, all-target Clippy with warnings denied,
and the optimized release build also passed.
The earlier phonebook-name correction is live-confirmed; this self-chat display
correction still awaits verification in the updated window.

## Live contact-name correction — 14 October 2026

The owner confirmed saved-session resume and receipt of a new phone message,
but the new self-chat showed a number while history chats had names. The adapter
read historical conversation titles and incoming push names but did not consume
phonebook app-state events. Own-device echoes also skipped the name path.

Heya now stores phonebook and profile names separately in encrypted archive
schema 2, subscribes to contact updates/removals, and resolves current names in
sidebar/header and group-message queries. Own-device profile names supply a
fallback for self-chat. Saved names win over profile names; stale mutations
cannot undo a newer rename/removal. Contact aliases merge with PN/LID identities,
and name-only records do not create empty chats. Existing history titles remain
fallbacks where no phonebook update is known.

The unchanged SDK exposes phonebook events and `resync_app_state` but stores no
phonebook-name mirror. Heya therefore requests the `critical_unblock_low`
snapshot when upgrading or recovering an unclean contact session, coalesces the
inline events and commits replacement atomically after a complete report.
Normal clean restarts use incremental events. No cache sizes or renderer
settings were changed; network and database work remain off the Slint loop.

Regression coverage includes a blank-push-name self echo through the real
durability hook, saved-name priority, Unicode, alias resolution, group sender
renames, removals/stale replay, a 2,048-contact burst, snapshot rollback on a
simulated disk error, and schema-1 upgrade/reopen with draft preservation.
The full suite passed 38 unit/UI tests and one integration test; the separate
desktop-keyring test remains opt-in. All-target Clippy passed with warnings
denied; the optimized build passed. The real archive opened successfully in the
new build after the owner closed the previous window. The repaired contact name
was subsequently confirmed by the owner; the self-chat display exception is
addressed above. Outgoing delivery remains unverified.

## Live session and text reliability — 14 October 2026

Saved accounts now resume on normal startup, while first use waits for Connect.
Offline archive opening, connection banners, and confirmed unlink/relink are
implemented. Relink resets only the protocol scope through the pinned SDK's
public API, after its store-release fence completes. The encrypted archive and
phone binding survive. A different account is rejected, revoked credentials are
marked for relinking across restarts, and incomplete remote unlink is reported.
The protocol revision and renderer configuration are unchanged.

The messaging actor saves drafts before new-chat navigation and clears the
submitted draft atomically with the outbox insertion. Failed navigation keeps
the visible text editable. Phone validation keeps the new-chat dialog open on
invalid input. Connection/error banners reserve layout space above the chat
header; containment tests cover narrow and normal window sizes.

Validation commands completed successfully:

```sh
cargo fmt --all --check
slint-viewer --check ui/app.slint
cargo test --locked --offline
cargo clippy --locked --offline --all-targets -- -D warnings
cargo build --locked --offline --release --features slint/unstable-wgpu-30
cargo test --locked --offline --bin heya storage::tests::desktop_keyring_encrypts_and_reopens_a_disposable_database -- --ignored --exact
```

The default suite passed **35 unit/UI tests and one integration test**, with the
desktop-keyring test ignored. That opt-in test was then run separately against
the host's Secret Service and passed: it creates/removes a disposable key and
reopens a temporary encrypted database. It does not touch the account keys.
The outbox test runs the actual worker against a synthetic transport and checks
offline queuing, rejection, timeout, cancellation, and a racing receipt that
must never be downgraded or retried. Session reset, archive identity retention,
draft persistence failure, missed edit notifications, and confirmation/cancel
controls are also covered with synthetic data.

The optimized non-demo app opened the owner's existing archive with `--offline`
and reached the offline state without a storage error. After the owner clicked
Connect, lifecycle diagnostics reached Connected using the existing session.
The session was preserved. Deliberate text send/receive is still awaiting
confirmation; automated tests did not connect to WhatsApp or send any messages.
Fresh phone QR scanning, network interruption and full restart checks
remain live validation gates. The prior synthetic renderer measurements below
are historical results, not new measurements of this build.

## Header, settings and theme validation — 14 October 2026

The source-derived UI alignment now includes the Chats header, collapsed avatar
rail, composer controls, settings layout and the reference's eight named palettes.
Light is the default; Omarchy-specific integration is deliberately excluded.
See [the source audit](ui-source-audit.md#header-settings-and-theme-audit--14-october-2026)
for dimensions, working preferences and explicitly unfinished settings.

The full suite passes 32 unit/UI tests and one integration test; the desktop
keyring test remains opt-in and ignored. After the final rail correction, the
combined Slint UI regression was rerun successfully. It covers real callback and
keyboard behavior, settings geometry, theme selection, coalesced persistence,
composer modes, modal focus, and bounded avatar-rail creation/selection. Slint
declaration checking, Rust formatting, all-target Clippy with warnings denied,
and the optimized build pass. These tests use synthetic content and temporary
files and do not establish live-account or native desktop compatibility.

The final release binary was exercised sequentially in light and dark mode with
the existing 27-second synthetic benchmark, using
`--features slint/unstable-wgpu-30` for rendering callbacks and no renderer
override. Both runs selected Skia/WGPU with Mesa RADV and completed successfully.
No compilation ran alongside them. Raw metrics, logs, binary/source hashes and
commands are in [the benchmark record](../prep/ui-settings-benchmark-2026-10-14.json).

| p95 frame interval | Light | Dark |
| --- | ---: | ---: |
| Scroll, 100 loaded messages | 17.67 ms | 18.94 ms |
| Resize, 100 loaded messages | 24.83 ms | 17.98 ms |
| Scroll, 10,000 loaded messages | 17.25 ms | 17.57 ms |
| Resize, 10,000 loaded messages | 19.01 ms | 18.18 ms |

Process RSS after large-history scrolling was 212.5 MiB in light and 212.2 MiB
in dark mode. These are one-run sanity checks, not a controlled performance
comparison. Earlier checks before the avatar-rail correction are retained in
the record too: their small-history dark scrolling reached 33.32 ms p95, showing
that this desktop workload can vary. The final runs do not prove that all
interactions maintain 60 fps. Compositor animation, input latency, long-running
account activity and native system-theme changes still require deliberate
desktop validation. No Skia cache limits or build features were changed here.

## UI containment and responsiveness — 14 October 2026

The owner reached the connected chat list, then reported overlapping previews
and slow window operations. The live archive supplied entire multiline message
bodies to fixed-height sidebar rows; the demo's first-line-only previews hid the
mistake. Neither `overflow: elide` alone nor the existing composer tests checked
this case.

The fix bounds sidebar summaries before UI delivery, preserves full messages and
drafts, and gives row text explicit single-line geometry plus content clipping.
The sidebar now retains immutable source rows and creates cached Slint rows on
demand, rather than rebuilding every row's strings for every snapshot. Changed
rows invalidate individually when the model length is stable. Snapshot streams
coalesce to one pending UI callback and release their watch lock before updating
Slint. Unchanged conversations do not schedule scrolling. Wallpaper uses one
native repeating image, with the same SVG size, tint and origin.

Validation: **29 offline tests pass** (28 unit/UI tests and one integration test);
the opt-in desktop keyring test remains ignored in the default suite. New checks
cover long multiline/Unicode previews, full archive/draft preservation, row-cache
edits/reordering/filtering, direct raw Slint input, bounds across repeated resizes
in both themes, and 100 queued updates becoming one callback followed by a
successful subsequent update. `cargo fmt --check`, all-target Clippy with warnings
denied, and debug/release builds are the build checks for this revision.

The native benchmark is explicit and synthetic:

```sh
SLINT_BACKEND=winit-skia-opengl cargo run --release --locked -- --demo --ui-benchmark
```

It opens a separate demo window for 27 seconds: 500 chats, initially 100 loaded messages
from the 10,000-message fixture, idle, 200 snapshots/second (unchanged and changing),
repeated resizing, then maximize/restore. It then scrolls the 100-message page,
loads all 10,000 messages, scrolls/resizes that timeline, and exercises composer
growth. It prints only timings/counts and exits.
It does not open account files, contact WhatsApp, or capture pixels. Use a quiet
machine; do not compare runs taken during compilation. `--ui-benchmark` requires
`--demo` and is incompatible with `--screenshot`.

On this KDE Wayland desktop, Slint 1.18.1 with FemtoVG, the unoptimized baseline
spent 16.85–20.03 ms p95 applying each snapshot. The optimized baseline was already
much better, but still rebuilt all sidebar strings. The comparison below uses
the same **release** profile on both sides, before/after the retained model and
native wallpaper tiling. The preview containment and callback coalescing fixes
were already present in the baseline; this is not a complete original-app vs
final-app comparison.

| Metric (milliseconds; p95 unless marked maximum) | Before retained model/tiling | After, two runs |
| --- | ---: | ---: |
| Apply unchanged snapshot | 3.54 | 0.20–0.23 |
| Apply changing snapshot | 2.39 | 0.20–0.44 |
| Render during changing updates | 4.29 | 2.81–6.52 |
| Render during resize | 16.03 | 12.05–12.50 |
| Timer lateness during resize | 5.74 | 3.35–3.55 |
| Render during maximize/restore | 26.48 | 14.93–27.28 |
| Maximum timer lateness during maximize/restore | 58.78 | 30.68–43.62 |

Render CPU measurements span Slint's before/after-render callbacks and exclude
buffer presentation and compositor animation. The 16 ms timer measures event
loop lateness; it is not a display/input latency measurement. Startup includes
cold resource setup and is excluded from the table. These runs demonstrate
cheaper updates and steadier resizing; maximize/restore still has outliers and
its render p95 varies substantially between runs. Rendering during changing
updates also varied; the repeatable gain is in snapshot application and resize
timing, not every rendering measurement. Minimize animation, real-account
load, scroll anchoring and end-to-end interaction latency remain explicit
acceptance checks, not claims based on this benchmark. No benchmark threshold
is enforced in CI because desktop timing depends on hardware and system load.

### Timeline implementation

`src/presentation_timeline.rs` now owns transcript virtualization. A prefix-sum
height index provides logarithmic seeks. Only the viewport plus a quarter-screen
margin (capped at 128 logical pixels on each side) becomes Slint components.
Nearby rows are measured with a hidden
instance of the same `TimelineItem` component used to display them; offscreen
rows retain cached heights or estimates. Width changes invalidate measurements,
then remeasure the nearby range. This keeps shaping work tied to visible content.

The scroll anchor is a message identity plus an offset within that row. Prepending
history, edits, removals and width changes restore that anchor synchronously.
If the anchored message is removed, the next surviving message takes its place.
Following the bottom is separate from reading older messages. There are no
30 ms scroll restoration timers. This preserves a within-message pixel offset;
it does not yet track a particular character through text reflow.

Message bubbles use the selectable text control's own wrapped preferred height;
the separate hidden wrapped-text measurement has been removed. The hidden
timeline measuring instance and displayed rows still share a single component
definition, so their geometry cannot independently drift.

Conversation payloads use shared immutable messages and transcript text. The
archive worker retains those allocations when content is unchanged. The UI
caches converted rows and reuses visible components when scrolling. Copying the
loaded transcript materializes clipboard text only on the explicit copy action.
Changed snapshots still reconcile the loaded message index in linear time;
archive queries and transcript formatting still run on the worker. This is not
an assertion that all application work is proportional to the viewport.

Additional tests compare 10,000 indexed heights/seeks against linear positions,
exercise anchors through prepend/edit/removal/empty states, and check actual
Slint component geometry across repeated widths with 10,000 loaded messages.
The toolkit test checks bounded materialization, unchanged updates, copy text,
bottom following, and history insertion/removal. It uses the testing backend;
native rendering and frame pacing are measured separately. This does not yet
validate moving between displays with different scale factors, native IME or
cross-message selection.

### ZapFast comparison harness

`tools/bench/zapfast_ui_speed.rs` is a standalone synthetic driver for the pinned
ZapFast source. In a **disposable copy** of that checkout, copy it to
`src/bin/heya_ui_speed.rs`, then build with
`cargo build --release --locked --features demo --bin heya_ui_speed`.
Run the resulting binary directly after all compilation has stopped. It uses
`App::headless` with a detached/offline backend and a fresh temporary data
directory, never the installed ZapFast account. The pinned reference checkout
and the installed application remain unchanged.

Both workloads use 500 chats, the same mix of short, wrapped, multiline and
Unicode message bodies, 100 then 10,000 loaded messages, 1200×800 logical windows,
80-pixel scroll input approximately every 16 ms, reversing direction every 60
ticks, the same resize path and
the same composer growth pattern. Each retains its own UI, text renderer and
scroll physics. The driver wakes egui from a background thread; Heya's script
uses a toolkit timer. Workloads are comparable, not identical event-loop probes.

Frame-interval measurements describe application frame cadence, not compositor
presentation or input-to-photon latency. ZapFast's eframe CPU measurement includes
UI work and rendering; Heya's renderer callback measurement excludes some
application/layout work, so those CPU numbers must not be divided into a speed
ratio. Both retain cached recent rows when expanding from 100 to 10,000 messages;
the second scrolling phase mainly revisits that warm range. It is evidence for
handling a large loaded model, not a traversal or cold-shaping test of all 10,000
messages. Composer cadence is also not directly comparable: Heya redraws when
the draft changes (approximately every 64 ms), while the egui driver keeps asking
for frames every 16 ms. No screenshots or account contents are captured.

### Renderer configuration

The normal Cargo configuration explicitly enables Winit, Skia/Vulkan,
Skia/OpenGL and the software fallback. With no renderer override, Vulkan is
preferred and OpenGL is the next startup surface candidate. It disables Slint's
default features, so Qt is not selected implicitly. Earlier configurations used
FemtoVG/OpenGL and then Skia/OpenGL. Measurements use the
release profile and select the renderer explicitly; the software renderer is not
the intended desktop path.
Slint 1.18.1's OpenGL window setup requests VSync. The checked launch environment
has no software-driver, scale-factor, slow-animation or debug-performance override.
The system GL query reports accelerated AMD Radeon RX 550 rendering with Mesa
26.2.4. Separately, the synthetic Heya processes' DRM metadata confirms `amdgpu`
at `0000:04:00.0` for both renderers, and Slint identifies release-mode OpenGL for
each. This rules out accidental software rendering in those runs.

Skia is a separate supported GPU renderer. Slint prefers it when compiled in.
Compare both renderers with the same application build and workload:

```sh
cargo build --release --features slint/renderer-femtovg
SLINT_BACKEND=winit-femtovg ./target/release/heya --demo --ui-benchmark
SLINT_BACKEND=winit-skia-opengl ./target/release/heya --demo --ui-benchmark
```

These commands do not install system packages or change the toolchain. The
comparison build adds FemtoVG's dependencies to the lockfile; use a disposable
checkout if preserving the production lockfile. A plain Cargo build without
the extra feature restores the ordinary renderer configuration.
For a separate startup diagnosis, `SLINT_DEBUG_PERFORMANCE=refresh_lazy,console`
prints Slint's renderer/build identification without forcing continuous repaint.
Do not use `refresh_full_speed` when checking idle redraw behavior.
See Slint's [backend documentation](https://docs.slint.dev/latest/docs/slint/guide/backends-and-renderers/backend_winit/)
and [performance diagnostics](https://docs.slint.dev/latest/docs/slint/guide/development/debugging_techniques/).

### Measurements and decision

The complete timings, hardware metadata, source hashes and renderer identification
are in [`prep/ui-speed-2026-10-14.json`](../prep/ui-speed-2026-10-14.json).
The host is KDE Wayland, a Xeon E5-2697A v4, Radeon RX 550 with Mesa 26.2.4, and a
3840×2160/60 Hz display at 1.6 scale. All timed runs use optimized binaries with
compilation stopped. Desktop timing still varied substantially, so these are
observations from this host, not portable performance guarantees.

The controlled renderer comparison used **one binary**, in the order FemtoVG,
Skia, Skia, FemtoVG, with `refresh_lazy,console` diagnostics enabled. To avoid
inflating the gain, the table uses the **better FemtoVG run** against both Skia
runs. The slower FemtoVG run remains in the raw record.

| Workload | FemtoVG render CPU p95 | Skia render CPU p95 | FemtoVG frame interval p95 | Skia frame interval p95 |
| --- | ---: | ---: | ---: | ---: |
| Scroll, 100 loaded messages | 14.06 ms | 11.55–11.62 ms | 18.18 ms | 17.43–17.94 ms |
| Resize, 100 loaded messages | 10.46 ms | 6.98–7.85 ms | 18.43 ms | 18.20–18.29 ms |
| Scroll, 10,000 loaded messages, recent rows cached | 7.53 ms | 4.81–4.84 ms | 17.44 ms | 17.55–17.67 ms |
| Resize, 10,000 loaded messages | 13.72 ms | 8.02–9.64 ms | 32.85 ms | 17.85–19.58 ms |

Skia reduces rendering cost in all four cases and gives steadier large-timeline
resizing. Its ordinary scrolling cadence is close to the better FemtoVG run;
lower rendering CPU does not automatically mean a higher displayed frame rate.
The Linux default at that stage changed to Skia/OpenGL. This is evidence for a better
renderer choice for this UI, not evidence that Slint is inherently slower than
egui. Other platform renderer choices still require their own tests.

There is a measurable memory cost. One additional matched pair using the same
dual-renderer binary recorded process memory via `/proc/<pid>/smaps_rollup`:

| Point in the synthetic workload | FemtoVG RSS | Skia RSS | Additional resident RAM |
| --- | ---: | ---: | ---: |
| Idle, 100 loaded messages | 112.5 MiB | 240.2 MiB | 127.7 MiB |
| After scrolling with 10,000 loaded messages | 115.6 MiB | 243.2 MiB | 127.6 MiB |
| After resizing the 10,000-message view | 115.6 MiB | 243.5 MiB | 127.9 MiB |

At the middle point PSS, which apportions shared mappings, was 82.2 versus
209.0 MiB, so the difference is not just shared-library RSS accounting. These
figures exclude GPU VRAM, are workload/window dependent and are not a universal
fixed Skia overhead. The memory-only runs overlapped final compilation and their
timing results are excluded. The smoother renderer costs about **128 MiB extra
resident RAM** in this sample; the choice prioritizes the owner's stated UI
responsiveness requirement. ZapFast uses eframe's `egui_glow` OpenGL renderer,
as selected by its pinned Cargo features, rather than Skia.

The ZapFast synthetic harness recorded p95 frame intervals of 17.83–33.07 ms for
100-message scrolling, 33.44–48.35 ms for resizing that page, 23.03–48.87 ms for
scrolling with 10,000 loaded messages and 33.41–51.41 ms for resizing it. The
drivers, scroll physics and UI contents differ, and the wide run-to-run variation
precludes a general speed-parity or superiority claim. Its source also contains
custom variable-height row caching and anchoring; those needs alone do not show
that either toolkit is a poor fit.

Two experiments were rejected: measuring an extra full viewport on each side
made the first timeline rewrite slower, and a separate cache of bubble geometry
did not demonstrate a benefit. The retained implementation uses a small overscan
and a single Slint component definition for measurement and display, without
that extra geometry cache. Shared message data, lazy row conversion and removal
of scroll-restoration timers address application design costs directly.

The 60 Hz acceptance gate remains open. Initial font/renderer setup still causes
first-frame stalls; maximize/restore has event-loop outliers. Native IME,
cross-display scale changes, cross-message selection, minimize animations and
real-account load were not established by these runs. The default offline suite
has **31 passing tests** (30 unit/UI and one integration), with the desktop
keyring test deliberately opt-in.

For desktop testing use `cargo run --release --locked`. Close the existing
instance normally first; building does not replace an already-running process.
The owner's connected instance was left running during these changes.

### Skia OpenGL versus Vulkan — 14 October 2026

The follow-up comparison keeps standard prebuilt Skia, its default caches and
all application code unchanged. Both paths use the same release binary, on the
same RX 550, Mesa 26.2.4, KDE Wayland and 4K/60 Hz/1.6-scale display described
above. One warm-up per backend precedes three measured runs each, in the order
OpenGL, Vulkan, Vulkan, OpenGL, OpenGL, Vulkan. No compilation runs alongside
the measurements. Full results, raw logs, driver evidence and source hashes are
in [`prep/skia-backends-2026-10-14.json`](../prep/skia-backends-2026-10-14.json).

Reproduce the timing workload with:

```sh
cargo build --release --locked --features slint/unstable-wgpu-30
SLINT_BACKEND=winit-skia-opengl WGPU_BACKEND=vulkan SLINT_DEBUG_PERFORMANCE=refresh_lazy,console ./target/release/heya --demo --ui-benchmark
SLINT_BACKEND=winit-skia-wgpu WGPU_BACKEND=vulkan SLINT_DEBUG_PERFORMANCE=refresh_lazy,console ./target/release/heya --demo --ui-benchmark
```

The diagnostic feature is needed because Slint 1.18.1 otherwise omits its
WGPU surface's rendering-notifier callbacks: a zero-frame report would be
missing instrumentation, not exceptional speed. Both comparison paths use the
feature. It is unnecessary for ordinary Vulkan use and leaves Cargo.lock
unchanged. `WGPU_BACKEND=vulkan` is inert for native OpenGL and explicitly
restricts the WGPU run to Vulkan. This Slint version does **not** recognize
`winit-skia-vulkan` as a renderer name. The verified Vulkan path is Skia's native
Vulkan rendering with WGPU managing the surface/device; it loads RADV. Both
processes show active `amdgpu` graphics work on PCI `0000:04:00.0`.

Values below are the median of three per-run p95 frame intervals, with the
minimum and maximum of those p95 values in parentheses. Lower is better.

| Workload | OpenGL | Vulkan |
| --- | ---: | ---: |
| Scroll, 100 messages loaded | 33.36 ms (32.99–33.46) | 17.57 ms (17.44–17.66) |
| Resize, 100 messages loaded | 18.17 ms (18.15–18.38) | 17.98 ms (17.95–18.02) |
| Scroll, 10,000 messages loaded | 17.56 ms (17.51–17.68) | 17.53 ms (17.45–17.95) |
| Resize, 10,000 messages loaded | 19.75 ms (18.42–20.81) | 19.54 ms (18.33–19.67) |

Vulkan consistently improves the small-history scrolling phase in this batch;
the other three phases are much closer. Earlier OpenGL runs in the previous
comparison achieved roughly 18 ms for small-history scrolling, so this is not
evidence of a universal twofold Vulkan speedup. The large loaded history still
scrolls through recently cached rows, rather than traversing all 10,000 messages.

At the end of the large-history scrolling phase, median RSS is **242.6 MiB for
OpenGL versus 211.9 MiB for Vulkan**. Median PSS, which apportions shared
mappings, is **188.9 versus 176.7 MiB**. Thus Vulkan saves about 31 MiB in RSS but
about 12 MiB in PSS; part of the RSS difference is shared-library accounting.
These are process-memory measurements, excluding discrete GPU VRAM. Samples
come from `/proc/<pid>/smaps_rollup` immediately after phase reports. Whole-process
CPU over the approximately 27-second workload is 10.09 versus 9.69 seconds
(median), a modest difference. No cache limits or Skia build features were tuned.

All six measured runs and both warm-ups completed without reported renderer
errors or fallback. Native visual correctness, input latency, minimize animation,
mixed DPI, media and live-account behavior remain unverified. Vulkan is a
promising candidate for the next deliberate desktop trial; that comparison
retained the OpenGL default. The ordinary release build was restored afterward.
To try Vulkan without the benchmark or diagnostic feature:

```sh
SLINT_BACKEND=winit-skia-wgpu WGPU_BACKEND=vulkan cargo run --release --locked -- --demo
```

### Vulkan preference and startup fallback — 14 October 2026

Following the comparison, the owner selected the standard Skia build with
`renderer-skia-vulkan` enabled. `renderer-skia-opengl` and `renderer-software`
remain enabled. With `SLINT_BACKEND` unset, Slint's default Skia surface factory
tries Vulkan first and OpenGL next when Vulkan initialization returns an error.
Further candidates include a generic WGPU retry and Skia's software surface;
there is no application-specific fallback implementation or cache tuning.

Two separate offline desktop smoke checks verified the ordinary release build:

- A normal launch, with no backend or graphics-driver overrides, selected Skia's
  WGPU/Vulkan surface and loaded Mesa's RADV driver.
- A second launch set `VK_DRIVER_FILES` and `VK_ICD_FILENAMES` to a deliberately
  absent ICD file **only in that child process**. Slint logged the failed Vulkan
  initialization and selected its OpenGL surface with Mesa EGL. Both windows
  remained running during the eight-second observation and were then terminated
  by the smoke-test harness. This does not test graceful shutdown or performance.

The evidence is recorded in
[`prep/skia-vulkan-startup-2026-10-14.json`](../prep/skia-vulkan-startup-2026-10-14.json).
The software fallback has not been failure-injected. Startup fallback does not
establish seamless recovery from a later GPU/device loss or driver crash.
Use the normal launch to retain automatic selection; the earlier explicit
`winit-skia-wgpu` comparison command chooses a particular surface path.

The feature update changes Cargo.lock to include 13 additional optional/platform
packages. No existing locked version was removed or upgraded, and WGPU 29 is
not in the active Linux dependency graph. The actual Skia feature set remains
unchanged: the standard prebuilt library already contains OpenGL and Vulkan.
For Vulkan benchmark frame timings, enable `slint/unstable-wgpu-30` as shown
above; ordinary desktop use does not need that diagnostic feature.

Validation after the feature update: release build and `cargo fmt --check`
passed; all-target Clippy passed with warnings denied; `cargo test --locked`
passed all 31 offline tests (30 unit/UI plus one integration). The desktop
keyring test remained intentionally ignored. The native startup checks above
provide the separate evidence for actual Vulkan selection and OpenGL fallback.

## Desktop keyring regression — 14 October 2026

The first deliberate pairing attempt failed before QR generation. The earlier
24 passing tests used an in-memory `SecretVault`, temporary SQLCipher databases
and Slint's test backend; they did **not** establish compatibility with the real
desktop keyring. Their locked/missing/malformed-key cases were injected failures.

A disposable test against KDE's `ksecretd` reproduced the integration fault:
keyring 3.6.3's Secret Service backend sends `set_secret` bytes as `text/plain`.
The provider rejects non-UTF-8 key bytes and can leave an empty entry. A read-only
probe found an empty Heya session entry and no account database files.

The production adapter now stores `heya-key-v1:` followed by 64 hex digits as
ASCII bytes, preserving all 256 key bits. Both sides use the byte API to avoid
platform-specific password transcoding (UTF-16 on Windows). Reads accept this format and existing
raw 32-byte keys. Recovery of an empty entry is allowed only before database,
WAL or SHM files exist; malformed nonempty entries and existing data are retained.

The ordinary suite tests the codec and recovery boundaries without a desktop.
The opt-in test below uses the **production desktop adapter**, a uniquely named
disposable entry under `org.heya.Heya.integration-tests`, and a temporary
SQLCipher database. It exercises empty-entry recovery, encrypted write, fresh
keyring-handle/database reopen and cleanup. It never opens Heya account entries
or connects to WhatsApp:

```sh
cargo test --locked --bin heya storage::tests::desktop_keyring_encrypts_and_reopens_a_disposable_database -- --ignored --exact
```

At this revision, **26 ordinary tests passed**, and the desktop integration test above
was explicitly run and passed against KDE `ksecretd`. It is ignored by the
ordinary suite because it needs an unlocked desktop service.

For read-only account-storage diagnostics without printing keys or content:

```sh
cargo run --locked --example keyring_probe
```

Real QR scanning, authenticated reconnect and send/receive remain separate
live-account checks. A passing keyring test does not establish those outcomes.

## Encrypted archive and text messaging — 14 October 2026

The next implementation slice connects direct/group text reception, history
import and sending to the existing Slint shell. It introduces `archive.db` with
its own Secret Service key (`org.heya.Heya` / `archive-v1`), schema version 1,
saved drafts, PN/LID reconciliation, paged views and a persistent outbox. It uses
the pinned SDK's inbound/history hooks and stable outbound message IDs unchanged.
Receipt evidence cannot be overwritten by a late send result. Interrupted sends
remain unconfirmed and are not automatically retried. Cached chats remain visible
after Disconnect; opening the archive on a new launch still requires Connect.

At this slice, the synthetic suite had **24 passing tests** (23 unit/UI tests and
one integration test). New coverage includes archive key separation/reopen, identity
mismatch, transactional rollback, duplicate/equal-time history, alias collisions,
mutation tombstones, expiry, staged-history cancellation/replay/failure retention,
outbox crash recovery and receipt races. A cold SDK client exercises the actual
durability hook and receipt mapper against synthetic stores without starting the
transport; an injected write failure is returned to the SDK and surfaced to the
supervisor. Slint checks cover captured send destinations, double-submit guards,
newer draft preservation, and delivery footer/composer bounds at four sizes.

No real account files, messages or media were used by these tests. Live pairing,
phone history delivery, send/receive, network interruption and restart still need
a deliberate smoke test. Media playback/attachments, full edit/delete variants,
chat-settings updates, outgoing read receipts, full-text search and account
removal/relink remain open. See the README for current controls and limitations.

`cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`,
`cargo test --locked` and `cargo build --locked` pass for this slice. SQLCipher
prints expected diagnostics during deliberate wrong-key rejection tests.

No system packages or toolchains were installed. Both reference checkouts remain
unchanged. ZapFast's draft previews and failure footers informed interaction
choices; the application logic is Heya's own implementation.

## Initial connection implementation — 14 October 2026

The adapter uses the existing whatsapp-rust pin without changes. It implements
explicit QR pairing, encrypted session reopen, connection/reconnect states,
cancellation and bounded shutdown. The session key uses the desktop keyring:
Secret Service on Linux, Keychain on macOS and Credential Manager on Windows.
Only Linux has been compiled here. A Secret Service provider (such as GNOME
Keyring or a compatible configured KeePassXC/KWallet service) must be running
and unlocked when connecting. No system packages or toolchains were installed
by the implementation work.

Automated validation remains offline: temporary SQLCipher databases, a fake
credential vault, synthetic QR payloads, lifecycle event mapping, and the actual
Slint pairing controls/layout at 1200×800, 800×600, 620×480 and 520×480 in both
themes. Screenshot export is restricted to `--demo`. No real account has been
paired during these checks. Session resume across an actual authenticated
restart, temporary network loss and desktop keyring prompts still need a
deliberate live smoke test. That initial connection-only slice did not yet implement the chat archive or
sending; the text/archive slice above now adds them. Account removal/relink is
still pending.

Validation: all 15 tests pass, including encrypted session reopen, missing-key
preservation, the OS file lock and SDK shutdown before transport startup. The
Slint checks verify scrolling to the pairing action at the minimum window size.
`cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings` and
`cargo build --locked` also pass. Both reference checkouts
remain unchanged.

## UI source correction — 13 October 2026

The initial visual prototype has been revised against the pinned ZapFast Rust UI
source. The [source audit](ui-source-audit.md) records dimensions, typography,
theme roles, provenance and remaining differences. Earlier screenshots below
are historical evidence for the first prototype, not current visual baselines.

Validation for this revision: Slint declaration check, `cargo fmt --check`,
`cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked`
(seven tests) and `cargo build --locked` all pass. The UI workflow test now checks
actual element geometry for multiline composer growth, six-line capping,
scrollable long drafts, shrinking on clear, and usable editor width at
1200/800/620/520 logical pixels. Sender-run coverage checks consecutive messages
and participant changes. No screenshots were taken or used for this correction.
Both reference clones remain unchanged.

A subsequent positioning regression check reproduced the composer/hint overlap.
The composer now uses a shared vertical layout, with explicit sidebar and tail
anchors. UI tests check absolute positions and non-overlap as well as sizes;
see the [source audit](ui-source-audit.md#positioning-regression-correction).

## Initial implementation evidence

The first offline prototype was built and checked on 13 October 2026:

- Slint 1.18.1 and pinned whatsapp-rust compile with distribution Rust 1.99.0.
  No protocol source changes or system package installations were needed.
- The resolved graph has one `libsqlite3-sys` version, 0.38.2, using SQLCipher
  with vendored OpenSSL. The shared `bundled` feature is pulled in by SQLCipher;
  it does not mean a second plain-SQLite provider is linked.
- Six automated tests cover draft and send routing, Unicode preservation,
  ordered history paging, filters, the actual Slint callbacks/background service,
  and the protocol store's per-connection encryption hook. The storage check
  creates a synthetic device, reads with the correct key, rejects a wrong key
  and an unkeyed connection, and exercises FTS5. SQLCipher's expected diagnostic
  during the wrong-key test is not a test failure.
- The native application launches and exits on the development desktop. Screens
  are also rendered with `slint-viewer`; screenshots are visual checks, not
  performance or clipboard interoperability measurements.
- Native captures cover the default dark window, light at 800×600 and the narrow
  sidebar at 600×600. The initial timeline lands at its newest message after
  layout; dark capture metrics are 619 logical pixels of content, 552.25 visible
  and an offset of -66.75. The dark/light previews used synthetic data only;
  these superseded prototype captures were removed during workspace cleanup.
- `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings` and
  the Slint compiler check pass. Tests and captures must be repeated when the
  relevant code changes.

M0 is still open for video/audio playback, file/image paste, cross-message drag
selection, native IME/clipboard behavior, and precise scroll anchoring/performance.
The demo keeps drafts and messages in memory. The live text/archive slice adds
keyring-backed protocol and archive persistence plus the text outbox, as detailed
in the current implementation evidence above.

The user installs system packages and tools. The commands below are suggestions
for the user to run; preparation does not install packages or switch toolchains.

## Local environment

Observed on 13 October 2026 on the Linux development host:

| Tool or library | Observed |
| --- | --- |
| Distribution | CachyOS, Arch family |
| Rust and Cargo | 1.99.0 |
| rustfmt and clippy | Available |
| CMake | 4.4.4 |
| Ninja, C compiler, pkg-config | Available |
| Python | 3.14.7 |
| GStreamer core, app, video and GL | 1.28.8 |
| GStreamer H.264 and AAC decoders | `avdec_h264`, `avdec_aac` available |
| GStreamer Good Plug-ins | 1.28.8-1 installed; `qtdemux` and `autoaudiosink` available |
| Wayland client / xkbcommon | 1.26.0 / 1.13.2 |
| ALSA / system SQLite | 1.2.16.1 / 3.53.4 |
| rustup | Not found; Rust is installed through the distribution |
| rust-analyzer package | 20260907-1.1; executable responds |
| cargo-nextest | 0.9.146; executable responds |
| wl-clipboard / xclip | 2.3.0 / 0.13 installed |
| Inter font | inter-font 4.1-1 installed |
| slint-tools | 1.18.1-1 installed; slint-lsp and slint-viewer both report 1.18.1 |

Verification after the user's package installation passes every check in
`scripts/check_environment.py --media`. This includes `qtdemux` (MP4 demuxing)
and `autoaudiosink` (automatic audio output), now supplied by the installed
`gst-plugins-good` package. To repeat the check:

```sh
python3 scripts/check_environment.py --media
```

See the official
[qtdemux](https://gstreamer.freedesktop.org/documentation/isomp4/qtdemux.html)
and [autoaudiosink](https://gstreamer.freedesktop.org/documentation/autodetect/autoaudiosink.html)
plugin documentation. These belong to GStreamer Good Plug-ins.

Run `python3 scripts/check_environment.py --media` for a fresh read-only check.
This checks availability, not successful Heya linking, display output, sound or
codec redistribution. It does not install anything. Native dependencies for
other platforms must be validated there.

The initial directory's `.git` entry is not a usable Git repository in this
session. Preparation files are present in the workspace but have not been
committed or pushed. Initialize/connect the intended repository when setting up
implementation; do not overwrite an existing repository's metadata.

## Suggested pacman and paru packages

Checked against installed packages, configured pacman repositories and AUR on
13 October 2026. Rust/Cargo, rustfmt, clippy, base-devel, Git, pkgconf, CMake,
Ninja, Clang, GDB, Wayland, xkbcommon and OpenSSL are already present. So are
GStreamer base/good/bad/ugly and libav plugins. The recommended Rust, Slint,
clipboard and font packages below are installed.

| Package | Purpose | Status or priority |
| --- | --- | --- |
| `gst-plugins-good` | MP4 demuxing, automatic audio sink and additional media support | Installed; media plugin checks pass |
| `rust-analyzer` | Rust completion, navigation and diagnostics in an editor | Installed and executable verified |
| `cargo-nextest` | Test runner with useful output and test execution controls | Installed and executable verified |
| `wl-clipboard`, `xclip` | Inspect and generate clipboard data on Wayland and X11 | Installed; clipboard interaction remains to be tested |
| `slint-tools` via AUR | Slint language server, viewer and translation extractor | Installed; language server and viewer versions verified |
| `inter-font` | Match the reference's Inter typography during UI development | Installed |
| `bacon` | Background Rust build/check feedback | Optional |
| `sccache` | Compilation cache | Optional once builds become frequent |
| `cargo-audit` | Check resolved dependencies against vulnerability advisories | Optional maintenance tool |
| `flatpak`, `flatpak-builder` | Linux packaging and sandbox testing | Later, during packaging |

For a fresh development machine, the corresponding install commands are:

```sh
sudo pacman -S --needed gst-plugins-good rust-analyzer cargo-nextest wl-clipboard xclip inter-font
paru -S --needed slint-tools
```

`slint-tools` is currently 1.18.1-1, matching the UI toolkit pin. It builds from
source and has additional build dependencies that paru will list. Prefer this
matching version over the currently older `slint-lsp-bin` package (1.17.0).
Sources: [Arch GStreamer package](https://archlinux.org/packages/extra/x86_64/gst-plugins-good/),
[Arch rust-analyzer package](https://archlinux.org/packages/extra/x86_64/rust-analyzer/),
[AUR slint-tools](https://aur.archlinux.org/packages/slint-tools), and
[Slint editor tooling](https://docs.slint.dev/latest/docs/slint/guide/tooling/manual-setup/).

Keep the installed distribution Rust for now. `rustup` is optional if per-project
toolchains become useful; its pacman package conflicts with the distribution
`rust` package, so switching is an intentional user action, not another item in
the install list. Source: [Arch rustup package](https://archlinux.org/packages/extra/x86_64/rustup/).

Slint's Rust runtime/build helper, whatsapp-rust, Tokio, rusqlite and GStreamer
Rust bindings come through Cargo. The AUR Slint C++/Python packages are not needed
for this Rust application. A system `sqlcipher` package is optional: the proposed
Cargo configuration bundles it, so installing it alone will not change linking.
Arch's library packages generally include their headers; do not translate Debian
package names into speculative `-dev` packages.

The host already has xdg-desktop-portal with GTK/KDE backends, KWallet, libsecret
and Noto Color Emoji. Check the active portal and Secret Service at integration
time; installed packages alone do not prove the services are working. No extra
keyring or portal installation is currently proposed.

## Dependency and build policy

[prep/dependencies.toml](../prep/dependencies.toml) records the preparation
proposal. The actual application dependencies are now in
[Cargo.toml](../Cargo.toml), with resolved versions in [Cargo.lock](../Cargo.lock).
Use the installed Rust 1.99.0. Keep the lockfile with the source; a lower Heya
MSRV has not been established. Received audio now uses the default-enabled
`media-gstreamer` feature and the system GStreamer runtime. Video remains an M0
spike.

Keep `slint` and `slint-build` at 1.18.1 together and pin whatsapp-rust by revision.
Select protocol features explicitly; Heya owns window lifecycle and shutdown,
so it does not need the library's CLI signal handler. Do not enable VoIP,
plugins, debug snapshots or PII tracing by default.

For the proposed SQLCipher build, avoid enabling
`whatsapp-rust/sqlite-storage-bundled`, and check Cargo's resolved feature tree
for plain SQLite versus SQLCipher and duplicate native links. The vendored
OpenSSL path may also require platform C build tools, Perl and make. The exact
resolved graph remains an M0 check.

GStreamer is an optional, default-enabled feature so UI/domain work can build
with `--no-default-features`. The actual lock uses the aligned 0.25 binding family
(`gstreamer` 0.25.4, `gstreamer-app` 0.25.2), tested against the installed 1.28.8
runtime. The older 0.23 block in preparation notes is superseded by Cargo.toml. UI rendering choice and media GPU sharing must be validated
together. Software rendering is a fallback candidate, not a promise of equal
video performance.

Cargo is the application entry point. If a dependency uses Ninja, let Ninja
choose its default parallelism; do not pass fixed job counts or set
`CMAKE_BUILD_PARALLEL_LEVEL` to cap it.

## Reference checkouts

Reference checkouts are optional and disposable. When needed, create ignored,
detached checkouts at the recorded revisions with the official GitHub repositories
as origin:

| Directory | Revision | Purpose |
| --- | --- | --- |
| `.references/whatsapp-rust` | `0dc38af07632f48f0cf6c66df285f1643fc22b95` | Inspect the public protocol APIs; use upstream unchanged where possible |
| `.references/zapfast` | `c73ce57522d28eff4da1a5cc9e3b6de0f27b0de0` | Visual reference only |

Heya uses the pinned upstream Git URL plus its committed vendored patches, not a
path dependency on these inspection checkouts. To reproduce the references in a
fresh workspace, use:

```sh
mkdir -p .references
git clone --filter=blob:none --no-checkout https://github.com/oxidezap/whatsapp-rust .references/whatsapp-rust
git -C .references/whatsapp-rust checkout --detach 0dc38af07632f48f0cf6c66df285f1643fc22b95
git clone --filter=blob:none --no-checkout https://github.com/crmne/zapfast .references/zapfast
git -C .references/zapfast checkout --detach c73ce57522d28eff4da1a5cc9e3b6de0f27b0de0
```

Slint's reference tag remains `v1.18.1` at
`372cf0ee5577c3dfec309a45e7b778ba4e81b734`; its examples were inspected in a
temporary checkout. Existing directories should be inspected rather than
replaced. These are reference checkouts, not Git submodules or source vendored
into Heya.

## Update procedure

For an upstream update, inspect the diff from the recorded SHA, particularly
protocol event shapes, persistence migrations and Slint input/rendering APIs.
Update the pin and lockfile together. Run adapter/storage tests, offline UI
checks and deliberate live smoke tests. Review copied-source attribution and
refresh the reference screenshots only when intentionally adopting a new design.
Do not let a moving default branch silently redefine the port's behavior.

## Mouse wheel / photo responsiveness follow-up — 15 October 2026

Evidence: `.prep/validation/photo-scroll-20261015/` (ignored, synthetic only).
The native winit input check on private Xvfb measured a single line event at
60.0 logical pixels with Slint's default and 119.82 with Heya's 120px setting.
Both had intermediate positions (7 and 11 samples respectively); Heya retains
Slint's wheel animation rather than jumping by the whole distance.

Validation before the final focus-preservation adjustment: 75 unit/UI tests
passed, one opt-in desktop-keyring test ignored, and the storage integration
test passed. The final focus adjustment also passed strict Clippy. The release
compiler initially crashed; rebuilding with `RUST_MIN_STACK=33554432` succeeded.
This is a build-process workaround, not a toolchain or runtime change.

The new 10,000-photo benchmark can exercise real SQLCipher reads, per-message
image decoding and animated wheel input:

```sh
HEYA_TEST_RENDERER=winit-skia-opengl HEYA_BENCHMARK_PHOTOS=1 \
HEYA_BENCHMARK_PHOTO_ARCHIVE=1 HEYA_BENCHMARK_WHEEL=1 \
scripts/ui-offscreen.sh --ui-benchmark
```

That run recorded scroll render-CPU p95 21.95ms / layout p95 0.12ms, and resize
22.71ms / 3.14ms, with 3–4 resident photo rows at phase boundaries. The equivalent
archive workload with immediate pixel steps recorded 24.26ms / 0.15ms for scroll.
These are Xvfb OpenGL **llvmpipe software-rendering** results, not native GPU or
compositor frame rates. Earlier fixture-only runs varied substantially: two old
runs had scroll p95 18.54–22.69ms; two new runs were 52.71–59.94ms before a third
returned to the earlier range. Those outliers prevent a clean renderer speedup
claim. Functional wheel distance, animation, bounded residency and incremental
photo publication are established; real desktop smoothness remains the owner's
check. No cache-size tuning or additional cache layer was introduced.

## Received audio implementation — 15 October 2026

Default feature: `media-gstreamer`; `--no-default-features` leaves the native
layout and domain/archive code available and reports playback as unavailable.
The installed runtime is GStreamer 1.28.8; core/app development libraries,
Ogg/Opus decoding, `scaletempo` and automatic audio output are already installed.
No system packages or toolchains were installed by this task.

Audio has a separate schema-7 SQLCipher blob table with mutation/deletion
triggers and guarded cache writes. Revision-3 display repair extracts audio
metadata from older encrypted envelopes. PN/LID duplicate merges preserve only
an exact matching envelope's cached bytes. View-once content is excluded.
The protocol SDK pin remains unchanged. Audio and photo transports share one
bounded authenticated streaming writer, including cancellation during the SDK's
blocking work. Cached audio opens offline; a new download requires connection.

Only an explicit click downloads/plays. There is one pipeline and one in-flight
request, at most 20 MiB compressed input, and no retained PCM or plaintext media
file. Commands to the player coalesce; generation checks discard stale playback
updates. The worker polls at 10 Hz while playing and stops publishing unchanged
paused state. Inactive rows have no timers or decoder instances. GStreamer is
initialized lazily on the worker, only when playback is requested. Output remains
silent in demos, benchmarks and unit tests (`fakesink`), so automation cannot
play sound on the working desktop.

The real Opus test exposed incorrect paused seeking with a push-mode source.
The final source uses GStreamer's random-access mode with exact requested byte
ranges, appropriate to fully buffered media. Seek/rate changes wait for
`ASYNC_DONE` before reporting their position. References:
[GStreamer appsrc](https://gstreamer.freedesktop.org/documentation/app/appsrc.html),
[seeking and position queries](https://gstreamer.freedesktop.org/documentation/application-development/advanced/queryevents.html),
[pitch-preserving scaletempo](https://gstreamer.freedesktop.org/documentation/audiofx/scaletempo.html).

Synthetic validation: 81 unit/UI tests pass, one opt-in real-keyring test is
ignored, and the encrypted protocol-storage integration test passes. New coverage
includes actual Ogg/Opus generation and decoding, WAV, pause, seek, 1.5×/2× rate
changes, end-of-stream, track replacement, malformed media, cancellation,
offline cache use, expiry during paused playback, old-schema migration, wrong-key
rejection and guarded late writes. Layout tests cover light/dark/narrow sizes,
fixed control bounds, 10,000 virtualized audio messages and unchanged timeline
identity/geometry during progress updates. Strict all-target Clippy passes;
`cargo check --locked --no-default-features` also passes.

These checks establish the synthetic decoding, storage and UI paths. They do not
establish the owner's real audio device, WhatsApp CDN compatibility or audible
quality on every codec. Video, recording/sending, played receipts, automatic
next-clip playback and disk-cache eviction remain pending.

Optimized offscreen evidence is in `.prep/validation/audio-20261015/` (ignored).
All runs used private Xvfb, OpenGL/llvmpipe, animated wheel input and synthetic
content; compilation had finished before each measurement. The screenshots
include light/narrow layouts, loading, and actual silent playback. The final
layout suite additionally checks long delivery-label wrapping and aligns the
short duration/timestamp baselines. The final playback screenshot exposed
implicit centering of the partial waveform fill; both the clip and its path now
have explicit left origins, with a regression assertion for their alignment
and width at half progress.

| Workload | Scroll render p95 | Scroll layout p95 | Resize render p95 | Resize layout p95 |
| --- | ---: | ---: | ---: | ---: |
| Text before audio support | 23.42ms | 0.41ms | 45.28ms | 21.20ms |
| Text with audio support idle | 23.30ms | 0.40ms | 53.09ms | 20.03ms |
| Initial dense audio, silent playback | 37.03ms | 0.45ms | 44.59ms | 0.44ms |
| Final dense audio, silent playback | 30.64ms | 0.54ms | 40.94ms | 3.91ms |

The final audio run instantiated 12–15 rows for 10,000 messages. The inactive
played-waveform overlay was removed, waveform scaling now preserves amplitude,
and duration/footer placement is fixed before playback. Ordinary text scrolling
was essentially unchanged in this comparison. Audio still costs additional
software drawing work; these results do **not** establish 60fps on the real GPU
or validate compositor animation. Resize variance and the changed row density
also prevent treating the table as a controlled renderer speedup claim. The
application's scroll layout remains below 1ms p95 in these runs. A real audio
output/account check is still the owner's next validation step.

## Message hover actions — 15 October 2026

The pinned ZapFast geometry is recorded in `ui-source-audit.md`. Heya adds a
matching Forward control beside React/Reply. The observer is an ancestor of the
message's existing inputs, so it does not cover selectable text, images, link
cards or the audio player. Only materialized visible rows instantiate buttons;
hover updates neither the timeline model nor its row heights. There is one
shared Slint action dialog, whose emoji results are capped at 120 and whose recipient
list is virtualized. Emoji glyphs use the platform's system color-emoji font.
No browser, per-row popup, polling timer, decoder or new cache tier was added.

Schema 8 stores reply drafts and individual reaction projections. The existing
outbox now carries opaque prepared envelopes for replies, reactions and forwards.
The protocol adapter uses the unchanged pinned SDK's public helpers. Business
cards forward as readable text, and media forwarding reuses supplied CDN
references. This does not implement expired-CDN re-upload recovery. Expired,
deleted and view-once sources are rejected; edits quote the current displayed
text, not the old envelope. Conflicting quote targets are cleared during alias
merges while draft text is retained.

Synthetic storage/adapter coverage includes durable quoted drafts, explicit
recipient ownership, read-only destinations, failure/uncertain delivery,
reaction replacement/removal and ordering, PN/LID merges, expiry/deletion,
current edited content, and schema-7 upgrade without discarding valid cached
media. Toolkit tests use actual pointer routing for hover controls and pickers,
keyboard activation, retained text/media input, wheel animation, clipped tall
messages, narrow light/dark layouts and bounded creation for 10,000 messages.
The full suite passes 89 unit/UI tests plus the encrypted protocol-store
integration test; the opt-in real-keyring test remains ignored. Strict Clippy,
format and no-media build checks pass. Real reaction/reply/forward delivery has
not been exercised automatically; account interoperability remains a deliberate
owner check. Aggregate reaction snapshots embedded in history are not yet read.

Evidence is ignored under `.prep/validation/hover-20261015/`. All screenshots and
measurements use `scripts/ui-offscreen.sh` and synthetic data. `--hover XxY`
positions the pointer for framebuffer captures. `HEYA_BENCHMARK_HOVER=1` keeps a
real X11 pointer over the synthetic transcript during the benchmark, alongside
`HEYA_BENCHMARK_WHEEL=1`; this does not touch the working desktop's pointer.
Early popup captures taken during compilation were inconclusive; isolated
repeat captures and the actual pointer/callback regression test verify that
the dialogs remain open and respond to input. Timing measurements below run
after compilation finishes.

The final hover buttons retain their keyboard/input targets while hidden, but
instantiate their rounded rectangle, shadow and icon only when revealed or
focused. This avoids keeping invisible drawing elements in each visible row.
Slint already skips fully transparent shadows; this is not a claim that the
previous implementation performed an invisible blur on every frame.

Optimized measurements use private Xvfb with OpenGL/llvmpipe, a stationary
pointer in the transcript, animated wheel input and 10,000 synthetic messages.
The before and initial-action builds were measured twice in opposite order;
ranges show those two runs. The final build was measured once per workload.
All timings below are milliseconds; resize refers to the 10,000-message phase.

| Workload/build | Scroll render p50 | Scroll render p95 | Scroll layout p95 | Resize render p95 |
| --- | ---: | ---: | ---: | ---: |
| Text, before actions | 14.34–14.38 | 21.08–23.24 | 0.37–0.42 | 46.23–47.00 |
| Text, initial actions | 14.70–14.92 | 29.72–30.72 | 0.40–0.42 | 40.23–42.24 |
| Text, final actions | 14.02 | 30.05 | 0.40 | 40.39 |
| Photos, before actions | 10.92–17.29 | 14.20–21.08 | 0.24–0.25 | 27.74–29.37 |
| Photos, initial actions | 11.01–11.91 | 19.20–38.33 | 0.28–0.57 | 31.81–32.56 |
| Photos, final actions | 10.60 | 15.89 | 0.27 | 24.17 |

The final photo scroll held four instantiated rows/images, and five during
resize. Toolkit tests also bound text-row/control creation independently of
history size and verify that hover preserves row heights and model identity.
Median text drawing and layout remain small, but the text render p95 is still
higher than the before build in this software-rendered harness. Photo variance
and the single final runs prevent a controlled speedup claim. These measurements
do not establish zero overhead, real-GPU frame rates, compositor responsiveness
or live-account delivery. Final synthetic screenshots cover the shared reaction
picker and its color emoji, quote composer, forward confirmation, and hover
controls on text, photos and audio.

## Sidebar photo summaries — 15 October 2026

Photo summaries now use structured metadata and the current group sender name,
following the display rules recorded in `ui-source-audit.md`. A shared bounded
formatter serves both live/archive and demo paths. The SQL query projects small
strings on the archive worker and joins one indexed latest visible message per
chat; it replaces two separate latest-message lookups. No Slint component,
image decode, media request, cache or timer was added. This is a query/formatting
change; no renderer speedup is claimed or inferred from it.

Validation: 91 unit/UI tests and the encrypted protocol-store integration test
pass; one opt-in real-keyring test remains ignored. Strict all-target Clippy,
format checking and the media-disabled build check pass. Added cases cover
photo metadata versus literal `[Photo]` text, caption preservation, first-line
previews, Unicode, draft priority, direct/outgoing messages, hidden events,
revocation, current profile/saved names, removal, PN/LID aliases and reopening.
The actual SQLCipher query plan is checked for use of `messages_timeline` rather
than a per-chat full-history scan. Light/dark synthetic framebuffer captures
verify the existing sidebar Text's layout. Evidence is ignored under
`.prep/validation/hover-20261015/sidebar-*`; all UI checks ran offscreen.
