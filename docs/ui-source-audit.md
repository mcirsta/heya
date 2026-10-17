# ZapFast UI source audit

The visual baseline is the cloned ZapFast revision
`c73ce57522d28eff4da1a5cc9e3b6de0f27b0de0`, not estimates from screenshots.
Dimensions below are logical pixels. ZapFast declares this layout in Rust/egui;
Heya implements it independently in Slint. No ZapFast application, storage or
protocol logic is used.

## Source mapping

| Element | ZapFast declaration | Heya implementation |
| --- | --- | --- |
| Sidebar | `settings.rs`: default 320; `ui/chats.rs::show`: range 260–520 | `Metrics.sidebar`; constrained further when the window shrinks |
| Single-pane breakpoint | `ui/mod.rs::NARROW_WIDTH`: 620 | `Metrics.narrow` |
| Both header rows | `conversation.rs::HEADER_ROW`: 44; horizontal margin 14, vertical margin 8 | 60 overall; same margins and aligned centers |
| Sidebar heading/avatar | `chats.rs::header`: bold 20; avatar 34 | Chats at 20; picture/initial avatar 34 |
| Conversation avatar/title/subtitle | `conversation.rs::header`: 40; semibold 15 and regular 12.5 with subtitle (17 without) | 40 / 15 / 12.5; demo always has an offline subtitle |
| Search | `widgets.rs::search_field`: height 34, radius 17, icon 16 at x=10, text x=34, font 14 | Same geometry, focus fill/outline and clear control |
| Filters | `widgets.rs::dotted_chip`: height 28, horizontal padding 9, medium 12.5; `chats.rs::filter_chips`: gap 4 | Shared `Pill`; six visible filters, with All/Unread implemented |
| Chat row | `theme.rs::ROW_HEIGHT`: 68; `chats.rs::row`: avatar 48 at (14,10), text x=76 | Same dimensions, full-row hit target |
| Chat title/time/preview | `chats.rs::row`: title 14.5 at y=14; time 11.5 at y=15/right inset 14; preview 13 at y=38 | Same typography/positions; timestamps explicitly right anchored |
| Row selection | `widgets.rs::row_highlight`: inset (8,2), radius 10; gentle shadow and raised edge | Same card bounds/radius; theme-aware edge and shadow |
| Unread badge | `widgets.rs::badge`: height 20; width max(20, text+12); semibold 11; on-accent ink | Same dimensions, growing width and 99+ cap |
| Header icon buttons | `theme.rs::icon_button`: icon size +12; headers request 18 | 30 hit area / 18 icon; hover changes ink |
| Bubble body/width | `conversation.rs::BODY_SIZE`: 14.5; `bubble`: min(available × .72,560) | Same font/cap; incoming group avatar column subtracts 36 |
| Bubble padding | `conversation.rs::bubble_frame`: left/right 10, top 6, bottom 5 | Same padding |
| Bubble shape | `widgets.rs::bubble_shape`: radius 10, tail width 8/height 11, top tail corner square | Same radii/tail dimensions; 3px overlap to avoid seams |
| Message grouping | `conversation.rs`: row gap 3; additional run gap 5; sender 13, avatar 28, column gap 8 | Same grouping by direction/sender; group-only incoming name/avatar |
| Timestamp | `conversation.rs::footer`: font 11; inline gap 8; otherwise footer height 15 + gap 4 | Same type/spacing; inline for unwrapped single-line messages (limitation below) |
| Date chip | `widgets.rs::chip`: medium 12; padding (10,5); half-height radius | Same; separator belongs to scrolling history |
| Composer panel | `conversation.rs::composer`: margins left/right/bottom 8, top 0 | Same |
| Composer pill | `composer_pill`: incoming-bubble fill, radius 24, inset 2 | Same; ~40px empty instead of a fixed 68px |
| Composer growth | `composer`: line=max(round(font line height+14),36); field grows to 6 lines, then scrolls | Font-measured growth/cap; controls stay beside the last line |
| Send button | `theme.rs::circle_button`: diameter=line; icon=diameter × .46 | Same size and inset; reference Lucide send SVG |
| Shortcut hint | `composer`: 11px text, 13px icon in 25px target, 8px gap above | Compact dismissible hint; Heya's actual shortcuts only |
| Wallpaper | `wallpaper.rs::paint_rect`: fixed 374×666 tile; `doodle_ink` tint | Fixed raster tile derived from the original SVG; window-anchored repeat; matching theme tint |
| Themes | `theme.rs::Palette`: panel, surface, hover, active, text, secondary, dim, accent, on-accent and bubbles | Separate semantic roles in `theme.slint`, including dark on-accent ink |
| Raised surfaces | `theme.rs`: 1px top edge, dark tint .128/light .48; shadow offset (0,2), blur 6, alpha 52 | Shared `RaisedSurface` and bubble styling |

Typography uses the installed Inter family. The local font lookup resolves Inter
Regular; weight 500/600/700 is requested explicitly where the reference does so.
The dark outgoing timestamp color is the result of ZapFast's `readable_on`
calculation for the pinned default palette (`#bcc5cb`), not the panel grey.

## Deliberate scope differences and remaining limitations

- Heya keeps its name, synthetic demo avatars and Copy loaded messages
  action in the conversation menu. The offline state is in the window title and conversation subtitle;
  the extra 55px sidebar footer from the first prototype has been removed.
- Attachment, emoji and microphone controls use the reference positions and
  icons but remain disabled until their behavior is implemented. Text sending
  works; an empty draft displays the disabled microphone.
- Sender names/avatars appear only at the start of incoming group runs. Direct
  chats and outgoing bubbles do not gain a redundant sender heading.
- Slint's plain selectable text does not expose the width of its final wrapped
  line. Wrapped/multiline messages currently use a separate footer; ZapFast can
  put it alongside a short last line. This remains a known density difference,
  not a claim of exact wrapped-text layout parity. Text remains selectable and
  copies without timestamp padding embedded in the message.
- The main header, menu, composer and settings controls now use the reference
  Lucide SVGs. These and the wallpaper are recorded in the notices.
- Demo messages have no protocol receipt state. No fake delivered/read ticks
  are added for visual similarity. Local sends retain their Local only label.

## Validation

`slint-viewer --check ui/app.slint` compiles the declarations. The Rust UI test
uses the actual Slint element geometry and font measurement to check empty,
multiline, capped and cleared drafts, and editor width at 1200/800/620/520px.
Existing callback tests also exercise draft restoration and local sending after
these layout changes. No screenshots were used for this revision's validation.

### Positioning regression correction

A bounds-based regression check reproduced a 20.5px downward displacement of the
composer pill and a 12.5px overlap with the shortcut hint at 1200×800. The earlier
checks measured sizes without checking positions; they did not detect this bug.
Slint was centering the freely positioned pill because its `y` was unspecified.

The pill and hint now share a `VerticalLayout`, with an 8px gap and 8px bottom
padding. A `HorizontalLayout` separates the hint's dismiss button and text. The
sidebar is explicitly anchored at (0,0), and bubble tails explicitly start at
the bubble's top edge. Regression checks cover actual absolute bounds, sibling
separation and containment at 1200×800, 800×600, 620×480 and 520×480, including
multiline drafts, capped scrolling drafts and a dismissed hint.

## Depth and interaction effects

These effects are explicitly painted by ZapFast; they are not automatic egui
panel styling. The source entry points below refer to the same pinned revision.

| Effect | Source declaration | Heya status |
| --- | --- | --- |
| Sidebar shadow onto conversation | `widgets.rs::paint_shadow_beside`, `conversation.rs::show`: 9px horizontal gradient, alpha 32→0, starting beneath the header | Implemented as a paint-only overlay; absent in the single-pane layout |
| Header shadow onto history | `widgets.rs::paint_shadow_below`: 9px downward gradient, alpha 32→0, with a 1px outline inside the header's bottom edge | Implemented above the scrolling content |
| Sidebar hairline | `widgets.rs::paint_edge_beside`: 1px inside the panel's right boundary | Corrected to sit inside the sidebar; does not consume conversation space |
| Surface lift | `theme.rs::bubble_shadow`: offset (0,2), blur 6, spread 0, alpha 52 | Used on bubbles, composer and date chips |
| Lit top edge | `widgets.rs::{raised,bubble_shape,chip}`: duplicate silhouette translated up 1px behind the original fill | Corrected from an inset strip to the actual raised silhouette; includes bubble tails |
| Chat hover/selection depth | `widgets.rs::row_highlight`: same shadow with alpha × .6 = 31, plus the raised edge | Implemented on chat cards |
| Bubble tail shadow | `widgets.rs::soft_triangle`: a mesh fading around the triangular tail, beneath its fill | Still pending; Heya has the body shadow and tail highlight, not that separate feathered mesh |
| Menus, toasts and pickers | `theme.rs::float_shadow`: offset (0,6), blur 20; `widgets.rs::menu_frame`: 1px outline, radius 8 | Used by the new settings picker and conversation menu; notices remain inline |
| Dialogs and image viewers | `theme.rs::modal_shadow`: offset (0,12), blur 40; `dialogs.rs::show`: radius 12 and dimmed backdrop | Used by the New chat modal; media viewers remain pending |
| Search/details overlay | `pane.rs::show_overlay`: leftward offset (-4,0), blur 16, outline at left edge | Recorded for the future overlay pane |
| Newest-message button | `conversation.rs::messages`: offset (0,3), blur 10, 40px circular button | Recorded for the future return-to-bottom control |
| Overflow fades | `chats.rs::filter_chips`: 16px right fade only while more chips are hidden; `widgets.rs::scrolling_text`: 24px text fades | Filter overflow fade implemented; long chat text still elides |
| Media timestamp backing | `conversation.rs::footer_over_picture`: rounded black backing at alpha 110, padding (6,2) | Recorded for media bubbles |
| Pointer feedback | `theme.rs::icon_button`: glyph scale .92 while pressed; `circle_button`: disc scale 1.05 on hover, fixed glyph/hit box | Pressed glyph scale implemented; circular hover expansion remains pending |
| Floating scrollbar | `theme.rs::install`: invisible at rest, handle alpha .55 active/.85 interacting, width 6 (8 while interacting), min handle 28 | Recorded; Heya still uses Slint's scrollbar styling |

For the default light and dark palettes, `lift_shadow` caps alpha at 50 before
applying each effect's multiplier. The panel gradient therefore has the same
alpha in both themes; popup/modal shadows instead use the full palette shadow
(alpha 140 dark / 50 light). Do not use the darker modal shadow for every bubble.

Panel gradients are drawn after the conversation contents, in the same order
as the reference (header, then sidebar), so they shade the content without
changing layout or becoming input targets. Their positions are included in the
existing bounds regression checks. Measurements and effects come from source;
no screenshots were used.

## Chat pictures and self-chat labels

ZapFast's `ui/widgets.rs::paint_avatar` draws the picture inside the avatar circle
and keeps a fallback until it loads. `account.rs::avatar` and
`backend/worker.rs::fetch_avatar` informed the on-demand request/cache behavior.
Heya implements its own viewport demand, encrypted thumbnail storage, generation
checks and Slint image handoff. The existing 34/48/40/36px account/list/header/rail
geometry is unchanged. It currently retains the initials fallback for both
people and groups; full-size viewing and group sender photos remain pending.

Self-chat titles follow `src/app.rs::chat_title` / `self_title` at the pinned
ZapFast revision: the account profile name plus `(You)`, or `You` without a
profile name. The saved phonebook alias applies to other contacts. Heya resolves
this from its own encrypted account metadata and canonical chat identity. The
fallback avatar takes uppercase alphanumeric initials from the first and last
name words, consistent with `src/util.rs::initials`; the `(You)` suffix therefore
contributes `Y` rather than a parenthesis. This is a display-rule reference,
not a port of ZapFast's application state or contact implementation.

## Connection screen

`ui/session.slint` uses the login card declarations from `src/ui/login.rs` at
the pinned revision: maximum width 460px, outer margin at least 12px, radius
16px, inner padding 32px, 64px brand badge, 28px title, 14.5px supporting text,
and shadow offset (0,16) with blur 48 and the full palette shadow alpha. The
background blends 10% accent into the window color at the top. Heya supplies
its own brand mark, connection controls and status wording.

The QR is a 256px square with a four-module white quiet zone encoded into the
image. The card uses a vertical scroll view when its content exceeds the window
height, rather than overlapping controls at 520×480. The content-driven card
height, square QR and ordering/containment of status, QR and action are checked
through actual Slint element bounds in dark and light themes. This view does
not display synthetic contacts. Existing cached conversations can also be opened
with `--offline`; active QR pairing temporarily displays this card instead.


## Text messaging interaction additions — 14 October 2026

The pinned `src/ui/chats.rs` draft preview and `src/ui/conversation.rs` failure
footer were consulted as interaction references. Heya independently shows a
saved draft in closed chats' previews, persists text before clearing the composer,
and gives interrupted sends an explicit unconfirmed state. No ZapFast behavior
or storage implementation was ported. Delivery footers wrap within their bubble;
UI tests check their bounds at 1200/800/620/520 logical pixels. The existing
composer sizes, hint placement and theme/shadow rules remain the baseline.

## Sidebar containment and wallpaper rendering — 14 October 2026

Live previews exposed a mismatch with the synthetic fixture: whole multiline
message bodies were being drawn in fixed-height rows. ZapFast's
`ui/widgets.rs::line` explicitly limits single-line labels, and its message
summary takes the first line. Heya now creates bounded single-line summaries for
both data paths, while retaining complete messages and drafts in the archive.
Titles/previews have explicit 20px/18px boxes and `max-lines: 1`. A clipping layer
contains row contents without clipping the selection shadow. Row height remains
68px, with the existing title/preview positions and fonts.

The UI regression injects long multiline and Unicode text, including directly
into the Slint model to bypass Rust normalization, then repeatedly resizes from
520×480 through 1920×1080 in both themes. It checks actual row/text bounds and
adjacent-row separation. No real account content is used.

The initial correction replaced the wallpaper grid with one Slint Image using
horizontal/vertical `repeat`. It missed explicit Image bounds: only the centred
intrinsic-size tile appeared. The 15 October correction below supersedes the
earlier claim that this covered the conversation.

## Wallpaper coverage and fixed texture — 15 October 2026

The pinned ZapFast `wallpaper.rs::{paint_rect,rasterize}` caches a 374×666 raster
tile, repeats it without stretching, anchors the pattern to window (0,0), and
clips it to the conversation. Heya now follows that appearance using a single
Image with explicit bounds covering the panel. Its negative local offset keeps
the tile origin at window (0,0) as the sidebar and connection banners change.

The bundled PNG derives from the unchanged reference SVG, retaining its size,
artwork and theme tint. Heya decodes it once on the startup blocking worker and
passes ready pixels to Slint before showing the window. In the pinned Slint
1.18.1 Skia renderer, this raster-image path avoids the SVG cache's target-size
dependency; resizing does not rerasterize the artwork. Skia repeats one cached,
tinted image through its image shader. There is no UI element per tile or
window-sized generated texture. See the development notes for validation and
measurement limits.


## Header, settings and theme audit — 14 October 2026

The Linux header now follows `ui/chats.rs::header` and `header_button_row`:
34px avatar, **Chats** at 20px bold, and 18px panel-left, square-pen and settings
icons in 30px targets separated by 8px. The permanent connection and phone-number
rows have been removed. Account controls live in Settings; New chat is a modal.
The 34px search, 28px horizontal filter strip, 16px right overflow fade, 148px
list start and 68px contact rows retain the source geometry. All six reference
filters appear; Private/Favorites/Groups/Channels remain disabled. Conversation
Search (disabled) and More replace the standalone Copy icon; Copy loaded messages
is in More. Composer plus/emoji icons are present with 22px glyphs in 34px targets,
but disabled. These supersede the earlier deliberate omissions above.

Settings follows `ui/settings.rs`: centered column capped at 640px, minimum side
margins 32px, top margin 24px, title 24px, section headings 18px, cards with 10px
radius and padding (20,16,20,6), rows with 14px labels and 12.5px descriptions,
and 40×22 switches. In narrower panes controls stack below their labels to
preserve containment. Search matches section names, labels and descriptions.
Sections retain the reference order: Appearance, Chats, Notifications, Privacy,
System, Account, Files, About.

Light is the default at both the declaration and persisted-preference layers.
Light, Dark, Follow system and the eight Fastframe v0.5.1 palettes used by ZapFast
are selectable. The shared JSON assets are bundled; no egui or Fastframe runtime
is introduced. Heya derives the reference's incoming/outgoing bubble colours,
readable outgoing metadata, date fill and doodle tint. Follow system uses Slint's
native colour-scheme reporting. Omarchy-specific integration is deliberately
excluded. Generic custom palette files remain unimplemented and are not claimed.
No palette cache settings are changed.

Theme, System/Inter font, Enter sends, doodles and shortcut-hint visibility work.
Font changes invalidate virtual transcript measurements while retaining its
scroll anchor. Future media, notifications, privacy, locking, tray/startup,
proxy, profile and download settings are visibly disabled. Account unlink is
available while connected, with an explicit confirmation dialog. Revoked or
unlinked sessions offer confirmed relinking with archive preservation. ZapFast's
self-updater, multi-account controls, call privacy, GIPHY provider key and log-file
button are omitted because Heya currently has no corresponding product/service
choice. Additional wallpaper colours/images, language, zoom and custom theme
loading appear as unfinished settings. No fake account privacy values are shown.

Heya's connection banner and dismissible notices reserve vertical space above
the conversation header only while needed; the connected, notice-free layout
keeps the reference's header alignment. Offline/error controls wrap rather than
covering chat content. Tests check header, timeline and composer containment at
1200×800, 620×480 and 520×480. Account confirmation dialogs use the same dimmed
backdrop and source-derived modal treatment as New chat, and disable underlying
composer input until closed.

Preferences contain only local UI choices. Parsing/validation and atomic writes
run away from the event loop; a coalesced worker saves the newest state. Invalid
or future settings files are preserved and an in-page notice explains that
changes are temporary. Demo mode neither reads nor writes these settings.


The empty conversation now follows `conversation.rs::empty`: a 72px brand disc,
24px heading, 14px prompt and 12.5px shortcut hint around the pane's vertical
center, with no inactive composer. Heya supplies its own name and shortcuts.
The composer uses the source's -4px gap between 34px plus/emoji hit areas and
6px from emoji's edge to the text; its empty state shows a disabled microphone.
Icon glyphs use the reference's .92 pressed scale. Collapsing the sidebar retains
the reference's 60px avatar rail: a 60px header, 52px rows, 36px avatars, unread
badges, hover names and a 3×22px selected-chat marker. It reuses the filtered chat
model with Slint ListView virtualization and keeps the panel edge and shadow.

Validation: 32 unit/UI tests and one protocol-storage integration test pass;
the desktop-keyring test remains deliberately ignored. The real Slint UI test
checks the header's 60px/106px geometry and 148px list origin, settings column
bounds from 520×480 to 1920×1080, label/control separation, real theme-popup
keyboard selection, live/demo entry points, disabled privacy controls, search,
modal focus, the bounded avatar rail and its chat selection, both sending modes,
all palette selections, font remeasurement with
scroll-anchor preservation, and the actual coalesced save worker on shutdown.
File tests cover absent, corrupt, unknown-theme and future-version settings and
atomic replacement. These are synthetic tests, not live-account validation.


Slint 1.18's Fluent `Palette.color-scheme` retains `unknown` after selecting
Follow system, while its resolved background tracks the desktop. The Heya theme
reads that public, solid background to distinguish the effective light/dark mode.
Explicit theme choices also set the standard widget scheme, so scrollbar and
AboutSlint styling agree with Heya. This avoids a private toolkit API or a second
D-Bus listener; the build remains pinned to Fluent/Slint 1.18.1.
## Link previews and business actions — 15 October 2026

Reference: pinned ZapFast `src/ui/conversation.rs::preview_card` and
`interactive_buttons`, with envelope shapes checked in
`src/backend/worker.rs::classify_base` and `worker/interactive.rs`.

Heya's native Slint preview uses a 6px card radius, 8px inset, 64px thumbnail
with 4px corners, 8px gap, 13.5px semibold title, 12.5px two-line description
and 12px domain. The reserved 88px card height leaves room for two description
lines with the configured fonts and remains stable while images load
or fail. This intentionally bounds title/description height; there is no embedded
web document. Standalone URLs are blue and underlined. Received action rows use
the reference's light/dark link colours (#027eb5/#53bdeb), 14px medium labels,
44px hit rows, a 1px separator and a 14px icon with an 8px gap. Heya keeps
two-line labels inside that fixed row instead of growing the vertical padding:
this bounds layout and makes the measurement independent of lazy child creation.
They share
the bubble's full width and bottom corners. Unsupported actions explicitly say
“Use phone”.

Only virtualized visible/nearby rows instantiate these elements. The existing
measurement component uses the same geometry as displayed rows. No new renderer
or protocol-library modification is involved. The independently written adapter
extracts bounded display metadata and allowlisted external/copy actions; Heya
does not implement ZapFast's reply machinery or application state.

## Conversation dates and spacing — 15 October 2026

Reference: pinned ZapFast `src/ui/conversation.rs` around lines 2036–2145,
`src/ui/widgets.rs::chip`, and `src/util.rs::{day_key,day_label,long_date}`.
Messages start a new day by local calendar date. Labels are Today, Yesterday,
weekday names for 2–6 days ago, and `Thursday, 1 October 2026` style full dates
otherwise, including future dates. Heya currently uses English labels, matching
its existing interface; translated labels are not implemented.

The list has a 10px top inset. A day boundary adds 8px above the chip and 4px
below it, in addition to the list's 3px item spacing (7px from the chip's bottom
to the next bubble). The chip is 12px medium text with 10px horizontal and 5px
vertical padding, centred with the existing raised surface style. Same-run
bubbles have a 3px gap; changing sender/side on the same day adds 5px (8px total).
A new day starts a fresh bubble tail/sender run without adding that extra 5px.
The bottom keeps the reference's 4px end spacer plus 10px list inset.

Heya computes local dates while reading the archive on its worker and uses the
same domain grouping rules for demo/live rows. Each date chip belongs to the
first message row of its day, including at a loaded-page boundary. Only nearby
day boundaries instantiate a surface and text; there is no calendar widget,
new scroll container, or web content. Cached labels refresh on a single minute
check when the local date changes, without invalidating measured heights.
Time-only bubble footers retain outgoing delivery status; copied transcripts
retain full calendar dates. Sidebar date formatting is unchanged in this change.

## Received photos — 15 October 2026

Reference: pinned ZapFast `src/ui/conversation.rs::picture`, `fit_picture`,
`frame_size`, the `Content::Image` branch (around lines 4734, 6164–6380), and
`src/image_preview.rs`. The visual reference caps ordinary pictures at 320×440
logical pixels, uses 6px image corners, reserves a frame while loading, places
captions underneath and overlays the timestamp for captionless pictures.
Offscreen rows reserve space without loading textures; activation opens a viewer.

Heya independently implements these controls in `ui/photo.slint` and the existing
message bubble. The picture is a plain Slint Image with contain fitting and a
44px centered status disc with a 22px download/spinner/retry glyph. The spinner
rotates one static SVG only while loading; text explanations remain in accessibility
and the viewer. This replaces the initial wide text banner. Frames retain the declared aspect where it
fits the bounds; minimum frame size is 120×90, narrowed with the bubble. Heya
keeps the 440px height cap even for pathological narrow/tall metadata rather than
letting the minimum width enlarge the row without a bound. Missing dimensions
use 4:3. Downloaded pixels do not change the reserved frame or scroll position.
Captions remain selectable and wrap below the image, with a 6px gap. The photo
frame has a 6px bubble inset. The sole open viewer uses native fit/zoom/pan and
Escape/close controls; it is absent from closed and inline message rows.

The download scheduler, cancellation, SQLCipher media cache and worker decoding
are Heya code, not ports of ZapFast's backend/cache implementation. No new copied
assets, external document renderer, widget framework or dependency was added.

## Mouse wheel and photo loading — 15 October 2026

The pinned ZapFast calls `fastframe_scroll::Scrolling::apply` and uses its
`WHEEL_STEP = 120` logical points (fastframe revision in `prep/upstreams.json`,
`crates/fastframe-scroll/src/lib.rs`). Slint 1.18.1's winit backend multiplies
`MouseScrollDelta::LineDelta` by 60; its Flickable already animates wheel movement
with a 180ms constant-deceleration curve. Heya now translates line-based wheel
input to 120 logical pixels at the window boundary, leaving pixel-based
touchpad input unchanged. Slint still owns animation, routing and scrollbars.
No ZapFast scrolling implementation or extra animation engine is ported.

The isolated `presentation_scroll` bridge uses the exact pinned Slint core event
type: public `PointerScrolled` hardcodes `Cancelled` and therefore skips wheel
smoothing. The winit hook preserves the `Moved` phase of ordinary wheel input.
This is an internal API dependency to revisit when upgrading Slint; a public
wheel-distance setting or phase-aware event would replace it. It does not change
renderer selection, SDK sources, system packages or process-wide input settings.

Photo presentation now publishes each decoded image independently, reuses images
still in demand, skips obsolete rows, and removes the fixed 25ms debounce. Archive
mutations still require revalidation before any publication. The decoded-image
working set stays bounded to demanded rows and the one open viewer; no additional
cache tier or cache-size preference has been added. The three status SVGs are
small original Heya assets, not copies of upstream application code.

## Received audio — 15 October 2026

The pinned ZapFast `conversation.rs::voice_player` (lines 7220–7455) provides
36px play/download buttons, 10px gaps, a 30px waveform with 2px bars on a 3px
pitch, a 44×20px speed chip, and 11.5px duration text. Heya follows those
measurements with native Slint rectangles, paths, text and input targets.
Its 64 waveform samples are sender supplied (bounded and normalized in the
protocol adapter), or a flat fallback. Two fixed paths draw the waveform and
played portion; there is no control per sample, no webview, no per-row decoder
and no per-row polling timer. A single application playback snapshot updates at
most 10 times per second while playing, independently of timeline measurement.

Heya uses click-to-download-and-play, cancel during loading, pause/resume,
waveform seeking (click or keyboard arrows), and 1×/1.5×/2× pitch-preserving
playback. The duration and timestamp share a line. Failed playback keeps the
same row geometry and gives the explanation in the application notice and
accessible label. There is no automatic playback or next-message chaining.

All transport, archive and GStreamer logic is Heya's own implementation.
ZapFast's audio engine and media cache were not copied. GStreamer is shared
infrastructure for later video work; no video playback is claimed by this slice.

## Message hover actions — 15 October 2026

Reference: pinned ZapFast `src/ui/conversation.rs`,
`reaction_affordance_rect` / `reaction_affordance` (lines 2745–2915), and
`floating_circle`. Controls are 26px circles with 15px Lucide glyphs, 6px from
the bubble and 4px apart. Incoming controls prefer the right, outgoing the left;
React can swap sides near an edge. The vertical anchor is the visible clipped
portion of the bubble, including quote/footer. Less than 26px visible hides the
controls. The hover region includes the gap so crossing to the buttons does not
hide them. Hover or keyboard focus reveals the controls; floating layers hide
the underlying actions. Light uses a white overlay, dark uses #233138, with the
reference's small shadow and hover colors.

Heya independently implements this geometry in `message-hover.slint`. Forward
uses a double-arrow glyph and sits 10px beyond Reply (6px more separation than
the React/Reply pair), with the same circle, shadow and tooltip behavior.
Reply/Forward only appear when they fit. The ancestor input observer
preserves text selection, media clicks and wheel routing. Hover does not replace
the timeline model or change row geometry; controls exist only on materialized
visible rows. There are no hover polling timers or persistent per-message menus.

There is one transient native picker for the entire window. React offers six
common emoji plus a bounded searchable list; Reply prepares a composer quote;
Forward requires a selected existing chat and explicit confirmation. The emoji
picker and forwarding dialog are Heya implementations, not a port of ZapFast's
application behavior. Quotes and reaction badges are plain text/rectangles;
opening one adds no media decoder or browser. The reply/forward SVG assets were
copied from the pinned reference; Forward adds a second arrowhead. Their Lucide
ISC notice is retained.

## Sidebar photo previews — 15 October 2026

Pinned ZapFast `model.rs::Content::summary` / `with_caption` uses `Photo`, adding
`: <first caption line>` when present. `ui/chats.rs` prefixes incoming group
messages with the sender's short name; `app.rs::person_name` prefers saved names,
then a profile/message hint marked with `~`. Outgoing and direct previews have
no sender prefix. Drafts take priority.

Heya shares its preview formatter between live and demo paths. It identifies
photos using structured metadata, never by matching `[Photo]` in ordinary text.
Sender names are resolved from the current contact table with the message hint
as fallback. The short label uses the first word of the stored display name;
separate multiword phonebook first-name fields are not currently retained.
Captions, transcript bodies and photo caches remain unchanged. The archive query
joins the latest visible message once per chat through the timeline index,
extracts bounded strings on its worker, and supplies the existing single-line
sidebar Text. There are no additional controls, image decodes or network requests.


## Message dropdown — 16 October 2026

Additional visual reference: the owner-supplied Windows package
`whatsapp_desktop/WhatsAppDesktop_2.2640.100.0_x64__cv1g1gvanyjgm`, specifically
`WhatsApp.Design/Styles/Material3/MenuFlyout.xaml`. Its native menu presenter uses
250px minimum width, 10px padding and 18px corners. Rows use a 40px minimum height,
15px labels, 16px icons and an 8px hover highlight radius. Heya follows these
measurements while using its selected theme colors. The package's chat is hosted
in WebView2; this native style does not establish the exact chat menu structure.
The owner's screenshots `Screenshot_20261015_235122.png` and
`Screenshot_20261015_235106.png` supply the chevron placement, detached reaction
strip, action ordering and lower separator. Private screenshots are not copied
into the repository or synthetic fixtures.

Heya implements the menu independently with Slint primitives: one conditional
window overlay, one small entry model created on opening, and no per-row menus,
media loads, webview or animation/polling timers. The bubble chevron appears only
on hover/focus and leaves message geometry unchanged. Text/caption copy retains
newlines and Unicode. Reply/react/forward use the existing captured-message
commands, with source/capability checks at activation; opening is inert.
Unimplemented actions, including Download, are disabled with explanatory tooltips.
Photo/audio and document/video/sticker placeholders are classified from protocol
metadata, never from a textual marker. Decoder revision 5 recovers that metadata
from encrypted envelopes once, in existing bounded background batches.

The menu clamps to the window, opens above when necessary, and scrolls internally
on short windows. Keyboard arrows/Home/End skip disabled actions; Tab accesses
individual buttons. Escape, outside click, chat changes and source removal close
it. The existing floating React/Reply/Forward controls remain available.

Validation for this slice: 93 unit/UI tests plus the protocol-storage integration
passed (the real-keyring test remains opt-in); the final UI input/layout test was
rerun after placement/focus adjustments. Strict all-target Clippy, formatting,
the media-disabled build check and optimized build passed. Synthetic screenshots
and logs are in `.prep/validation/menu-20261016/`. Input tests exercise both
orientations and themes, four viewport sizes, text/photo/audio, document metadata,
copy, quick reactions, reply/forward routing, keyboard traversal, outside dismissal
(including the reaction-strip overhang), offline restrictions, changed/deleted
sources and a busy command queue. No live messages were sent by these checks.

Optimized, offscreen 10,000-photo workload with a stationary hover pointer:

| Measurement | Previous build | Menu build |
| --- | ---: | ---: |
| Skia CPU timeline scroll layout p95 | 0.24 ms | 0.25 ms |
| Repeated OpenGL scroll render CPU p95 | 18.02 ms | 18.80 ms |
| Repeated OpenGL resize render CPU p95 | 28.20 ms | 27.16 ms |
| Repeated OpenGL scroll frame interval p95 | 33.78 ms | 31.44 ms |

Both builds retained four inline photo rows/images during the 10,000-row scroll
phase; the closed menu creates no menu controls. The repeated OpenGL pair showed
no large reproducible regression. Initial OpenGL runs were much slower and
inconsistent (scroll render p95 59.78 / 109.50 ms); concurrent linker activity was
observed, and the unchanged baseline subsequently measured 18.02 ms. All runs
are retained, not treated as an improvement claim. The private Xvfb/OpenGL path
and its cold shader startup do not measure the owner's Wayland/Vulkan GPU or
compositor. Skia CPU provides layout timings, not render/frame counters. Native
frame-rate acceptance remains separate.

## Received document cards (2026-10-16)

The owner's WhatsApp screenshot guided the shallow preview above a file-information
panel. No private screenshot or document was copied into assets. The pinned
`whatsapp-rust/waproto/src/whatsapp.proto` DocumentMessage fields supply
`fileName`, `title`, `mimetype`, `fileLength`, `pageCount`, `jpegThumbnail` and
caption; none is inferred by opening the attachment. ZapFast's pinned
`src/backend/worker.rs` (`thumbnail_of` and document extraction) and
`src/ui/conversation.rs` document branch were inspected as context. The supplied
Windows package hosts web chat, so its native XAML was not treated as the source
of this document-card layout.

Heya uses an independently written `DocumentCard`: at most 352px bubble width,
6px inner inset, a top-aligned clipped preview up to 140px tall, and an 84px
information panel. Filenames use two bounded lines; available page count, type
and decimal byte size share a separate metadata line. PDF icons are red;
other files use the theme accent. Captions, forwarded labels, dates, timestamps,
and existing message menus retain their usual behavior. Copy caption works;
Download stays disabled. Unknown fields are omitted; missing thumbnails give
compact cards and corrupt thumbnails keep a fixed-size icon fallback.

The document itself is never downloaded, parsed or rendered. The existing
embedded-thumbnail worker now preserves aspect ratio and orientation rather
than cropping to an avatar square. Input remains limited to 256 KiB, JPEG/PNG,
2048px dimensions and a 32 MiB decode allocation; decoded pixels are at most
480px on their long edge and never upscaled. The unchanged demand limit is 32
nearby messages. The virtual timeline and image identity reuse remain in place;
image arrival/removal cannot change card geometry. Decoder revision 6 recovers
metadata from encrypted envelopes once. Absent document metadata is omitted
from serialized content so existing photo/audio cache fingerprints survive.
Full document filenames/captions are preserved in copied transcripts, including
older rows whose stored placeholder body omitted the caption.

Validation: 96 unit/UI tests and the encrypted protocol-storage integration
passed; the real desktop-keyring test remains opt-in. After the visual check
caught an implicitly centered preview overlapping its metadata, the preview got
an explicit origin and clip, and placement assertions were added. The final UI
suite passed again, including document caption menus. Strict all-target Clippy,
formatting, the final media-disabled build check and the optimized build passed. Light, dark and 520px-wide screenshots
were rendered and inspected offscreen. All inputs were synthetic; no documents
were downloaded from the owner's account and no messages were sent.

Optimized sequential Xvfb runs (artifacts in
`.prep/validation/documents-20261016/`):

| Existing 10,000 message-card workload | Before | After |
| --- | ---: | ---: |
| Skia CPU scroll layout p95 | 0.09 ms | 0.10 ms |
| Skia CPU resize layout p95 | 3.40 ms | 3.42 ms |
| OpenGL scroll render CPU p95 | 16.63 ms | 16.59 ms |
| OpenGL resize render CPU p95 | 15.17 ms | 15.62 ms |
| OpenGL scroll frame interval p95 | 21.76 ms | 21.79 ms |

OpenGL "Before" uses the repeated baseline, which exited successfully. Its first
run produced all phase metrics (scroll render p95 15.83 ms) but returned status
143 after reporting; that log is retained. The new-build runs exited normally.
These measurements show no substantial repeatable regression in the existing
card workload, rather than claiming zero extra work or faster rendering.

The separate 10,000-document workload uses the real encrypted thumbnail worker
and animated wheel input. OpenGL scrolling materialized four rows with three
loaded thumbnails; layout p95 was 0.14 ms, render CPU p95 17.39 ms and frame
interval p95 21.49 ms. CPU rasterizer scrolling materialized five rows/three
thumbnails with layout p95 0.08 ms. Resize layout p95 was 1.54 ms (OpenGL) and
1.18 ms (CPU). Card heights and scroll offsets remain identical when images
arrive or are removed. Missing, corrupt, oversized and view-once inputs,
archive upgrades/deletion/expiry and cache preservation are covered in tests.

Xvfb/OpenGL here reports unavailable DRI3 acceleration. These are isolated
render/application measurements, not native Vulkan/Wayland, compositor animation,
live document compatibility or a 60-fps acceptance claim. CPU rasterizer render
and frame counters are unavailable. No additional PDF or office libraries were
introduced; saving/opening documents remains pending.

## Conversation header presence — 2026-10-16

At the pinned ZapFast revision in `prep/upstreams.json`,
`src/ui/conversation.rs` uses a 40px avatar, 14px horizontal/8px vertical header
insets and a 17px semibold name when there is no subtitle, vertically centered
against the avatar. Its presence subtitle uses the accent color for online and
secondary text for a supplied last-seen timestamp. Heya now uses that larger name
on one row and, as requested, places 12.5px presence text immediately to its right
with a 12px gap. Two plain Slint Text elements replace the old title/subtitle pair.
Long names and statuses elide within the available header width; the name has
priority, and very narrow headers omit the status. No controls were added to
message rows, and the generic “WhatsApp conversation” label was removed.

The implementation is independent. The unchanged pinned whatsapp-rust public
presence API supplies subscriptions and events. One worker resolves only the
open direct chat's canonical PN/LID addresses on the archive worker, subscribes
serially, and removes the old subscription before adding another. A canceled or
timed-out attempt retains its peer for cleanup; failures back off for 30 seconds.
Subscriptions are released for hidden conversations and excluded for self-chat,
groups and read-only/unsupported addresses. Reconnect clears old availability
and resubscribes. Network and database work never run on the UI thread.

One transient watch slot coalesces events, discarding unrelated contacts and
identical updates. The UI applies only two header properties through the existing
coalesced event-loop handoff; it does not rebuild sidebar or timeline models.
Presence is not archived or logged. Only a timestamp supplied by WhatsApp is
shown, in local time; no offline time is inferred from messages or the local
clock. Unknown/private presence displays no status. A single background minute
tick updates calendar labels and picks up address merges, publishing only if
changed. The account's existing own-presence policy is unchanged.

Synthetic tests cover timestamp validity/calendar boundaries, private last-seen,
PN/LID matching, unrelated events, duplicate bursts, chat switching, cancellation,
timeout/backoff, reconnection and shutdown. Toolkit tests cover horizontal
placement/containment at 520–1920px widths, long Unicode names, stale navigation
updates, coalesced hide/show, and 1,000 status changes retaining message/timeline
models and viewport geometry. These tests do not establish live-account presence
availability, which remains controlled by WhatsApp's visibility rules.

Validation: 109 unit/service/toolkit tests and one encrypted protocol-storage
integration passed; the real desktop-keyring test remains opt-in. After review
caught a watch wake-up race, the five presence tests passed again, followed by
strict all-target Clippy and the media-disabled check. Light online, narrow
last-seen and dark group headers were rendered and inspected on private Xvfb
screens. No owner-account presence requests or desktop captures were made.
Artifacts are under `.prep/validation/presence-20261016/`.

The optimized build passed. Sequential before/after runs of the existing
10,000-message-card benchmark with Skia/OpenGL on private Xvfb gave:

| Measurement | Before | After |
| --- | ---: | ---: |
| Scroll layout p95 | 0.19 ms | 0.17 ms |
| Scroll render CPU p95 | 16.22 ms | 16.40 ms |
| Scroll frame interval p95 | 21.09 ms | 21.31 ms |
| Resize layout p95 | 3.81 ms | 3.52 ms |
| Resize render CPU p95 | 14.85 ms | 14.87 ms |
| Composer render CPU p95 | 16.24 ms | 14.21 ms |

Both processes exited normally; compilation had finished before either timed
run. This single comparison shows similar scrolling/resize cost, not a proved
speedup or zero additional work. The benchmark contains synthetic static header
presence; update bursts are covered by the coalescing/model-retention tests.
Xvfb reports unavailable DRI3 acceleration, so these results do not establish
native Vulkan/Wayland, compositor animation or end-to-end account performance.
The two idle frames include roughly two seconds of cold startup rendering in
both runs; those startup samples are not steady-state frame timings.

## Document preview resolution — 2026-10-16

The owner's blurry cards exposed a gap in the original preview implementation:
`jpeg_thumbnail` can be only a few dozen pixels wide. Enlarging it inside a
340px card cannot recover detail. The earlier synthetic page was already large,
so it did not reproduce this failure. The pinned ZapFast document branch uses
an attachment row, not a higher-quality first-page strip; the desired strip
comes from the owner's WhatsApp web/Windows references.

Heya now adapts `DocumentMessage.thumbnail_direct_path` and its two thumbnail
hashes plus media key to the SDK's authenticated streaming downloader. This
needs the additive `MediaType::DocumentThumbnail` patch tracked in
`patches/README.md`; the reference checkout and other SDK crates stay at their
recorded revision. The PDF itself is neither downloaded nor parsed. No PDF
engine, webview or new UI control is introduced.

Nearby message demand is bounded to 32 IDs. One serial worker requests image
previews, sharing the existing media semaphore, timeout and cancellation flag.
Navigation/disconnection/shutdown cancel work; requests with invalid metadata,
view-once content, corrupt/oversized data or expired CDN references fall back to
the embedded preview. Failed attempts persist a one-hour retry cooldown (retried
on later demand/connection activity, with no polling timer). Downloaded JPEG/PNG
bytes are limited to 1 MiB, dimensions to 4096 per side and decoder allocation
to 64 MiB. After orientation correction the worker retains only a top strip of
at most 768×320 RGBA pixels, under 1 MiB per resident image. The encrypted
schema-10 cache stores the derived strip, with stale-envelope, deletion and
expiry guards; mutations discard it.

A monotonic cache identity replaces the old assumption that thumbnail pixels
never change. Receipt-only updates reuse the same pixel allocation and Slint
image. The existing Image receives the sharper texture without changing row
height, timeline model or viewport offset. No animations or periodic repaint
work are added. The demo and document benchmark now use a larger synthetic
page, and regression tests explicitly replace a 32px embedded image with the
high-resolution strip. Preview quality still depends on the sender supplying
an accessible higher-resolution thumbnail.

Validation: 115 Heya unit/service/toolkit tests and one encrypted protocol-store
integration passed; the real-keyring test remains opt-in. The patched SDK's 40
download tests passed. Format, strict all-target Clippy, media-disabled check and
optimized build passed. Light 1200×800 and dark 620×800 synthetic document
screenshots were rendered and inspected on private Xvfb. The output binary grew
by 190,560 bytes; no new runtime library was introduced.

Sequential optimized before/after document benchmarks ran after all compilation
finished, using Skia/OpenGL on private Xvfb. The newer fixture supplies 768px-wide
strips rather than the previous 360px-wide synthetic page:

| 10,000-message workload | Before | After |
| --- | ---: | ---: |
| Scroll layout p95 | 0.13 ms | 0.13 ms |
| Scroll render CPU p95 | 14.25 ms | 15.14 ms |
| Scroll frame interval p95 | 18.23 ms | 19.64 ms |
| Resize layout p95 | 1.48 ms | 1.47 ms |
| Resize render CPU p95 | 13.08 ms | 12.65 ms |
| Composer render CPU p95 | 11.87 ms | 13.64 ms |

The timeline remained virtualized, with five instantiated rows and three loaded
previews during scrolling. This single pair shows comparable cost with more
image detail, not zero extra work or a guaranteed frame rate. Xvfb lacks DRI3
acceleration; it does not measure native Vulkan, compositor animation or live
CDN behavior. Artifacts: `.prep/validation/document-preview-20261016/`.

The tested binary was installed through `scripts/install-desktop.py`; installed
and built SHA-256 both matched
`3a2077df3309af66b443e5771ebd58cdad6783fa98042a0e38aa39b50d934977`.
The normal desktop entry reopened without `--offline` and reached Connected.
No account message was sent or screenshot captured. The owner still needs to
confirm the particular documents have accessible HD previews.

## Draft preview visibility — 16 October 2026

Primary reference inspected first: the supplied Windows package
`WhatsAppDesktop_2.2640.100.0_x64__cv1g1gvanyjgm`.
`WhatsApp.Design/DebugFeatures/WaAdminPage.xaml:295–297` explicitly describes
its draft-preview feature mask, including `8 - hide for current chat`.
IDA inspection of `WhatsApp.Root.dll` (SHA-256
`150a692e7498c046ebf6cef39f9994bdeec2772c53172399d0f827e420579d17`)
found the `DraftPreviewInChatList` accessors at `0x59f00` and `0x59f10`.
These are plain field accessors, not the rendering predicate. The bootstrap
state machine at `0x695b0` initializes WebView2 and the web-content updater;
`MainWindow.StartNavigation` at `0x34f00` navigates to the configured web URL.
This establishes that the package exposes this behavior, but does not establish
which flag is enabled in the current remotely supplied chat UI.

Secondary reference: pinned ZapFast `src/app.rs::draft_preview` at line 2531
explicitly returns no draft preview for `open_chat`. Heya independently applies
that rule in its shared sidebar projection, used by both live archive and demo.
Typing keeps the selected chat's last-message preview; leaving shows the saved
draft; returning restores the full composer text and hides the draft preview.
Autosave, shutdown flush, outbox transactions and draft-version fences are
unchanged. No additional UI controls, timers, workers or network requests are
introduced. Unchanged sidebar rows retain their existing presentation cache.

Validation: 123 application/service/UI tests plus one protocol-storage integration
passed; the real-keyring test remains opt-in. The archive regression checks open
versus closed previews, encrypted reopen, long multiline Unicode drafts, empty
conversations, switching between two drafts, clearing and photo captions. The
Slint callback test checks sidebar previews when leaving/returning and restored
composer content. The domain/archive tests assert identical sidebar output while
the open draft changes, so the existing row cache is not invalidated by those
edits. All-target strict Clippy and formatting passed. Tests use synthetic data
and the headless toolkit backend. No renderer/layout controls changed; these
checks establish preview/caching behavior, not a new compositor timing result.
Logs: `.prep/validation/draft-preview-20261016/`.

## PDF preview correction and transfer audit — 16 October 2026

The earlier HD-thumbnail-only approach was insufficient. A read-only diagnostic
of the affected account's document metadata found a 51×72 embedded image and no
separate thumbnail reference for the newly reported PDF. Another cached preview
was only 340×141. Enlarging either cannot restore text detail. No private document
content, names, IDs, keys or CDN paths were written into the diagnostic output.

The supplied Windows application's native download/save and share paths were
inspected in IDA. Evidence and function addresses are in
[the desktop media audit](desktop-media-audit.md). This package hosts
`web.whatsapp.com` in WebView2; its native assemblies do not establish the exact
PDF-card rendering algorithm. Heya's renderer is an independent implementation.

Nearby PDF cards now prefer a sufficiently large authenticated thumbnail, then
fall back to downloading the original PDF (maximum 16 MiB) and rendering the
first-page strip. Poppler runs in a short-lived isolated process, with no account,
network, display or session-bus access on Linux. The subprocess has an 8-second
wall timeout, 5-second CPU limit and 512 MiB address-space limit, plus a 64 MiB private temporary-filesystem cap. Input/output use
pipes; disposable font caches can only be written in private tmpfs, and the
system font cache is reused read-only. Work is serial and cancels when the card
leaves demand. No dedicated idle renderer thread, viewer control or UI timer was
introduced. The existing image control displays the 768×320 strip; originals are
not persisted by this feature.

Archive schema 11 retains older pixels while making tiny/failed previews eligible
for an immediate upgrade. A successful replacement receives a new image revision;
a failed upgrade preserves the old pixels and backs off for an hour. Decoder
revision 9 repairs older document metadata, including PDFs with no embedded image.
The SDK's existing authenticated document-download API supplies everything needed;
no additional protocol patch was required.

Validation: 127 application/service/UI tests plus one protocol-storage integration
passed; the real-keyring test remains opt-in. New checks cover real portrait,
landscape and rotated PDFs, fine vector detail (over 400 black/white transitions
that a stretched thumbnail cannot retain), first-page cropping, invalid/oversized
input, output limits, timeout and cancellation cleanup. Rendered pixels also pass
through the encrypted archive and image presenter unchanged. Migration checks
cover schemas 9 and 10, preserved drafts/old pixels, retry eligibility and cache
replacement. The real-render test exposed and fixed both a disposable-cache file
limit problem and rotated-page scaling. With read-only font-cache reuse, the
synthetic render/cache test took 610 ms initially and subsequent PDF renders
224/210 ms; these are local subprocess timings, not network latency or UI frames.

The demo/benchmark document fixture now uses an actual render of the synthetic
PDF rather than drawn imitation lines. Artifacts and logs are in
`.prep/validation/pdf-preview-20261016/`. No account message was sent by the tests.

Final checks: strict all-target Clippy, formatting, the build without default
features and optimized build passed. Private-Xvfb OpenGL captures were inspected
in light mode and dark mode at scale 2, using the actual synthetic PDF strip.
No owner desktop captures or visible performance windows were used.

| 10,000-message cached-document workload | Before | After |
| --- | ---: | ---: |
| Scroll layout p95 | 0.13 ms | 0.15 ms |
| Scroll render CPU p95 | 15.88 ms | 16.10 ms |
| Scroll frame interval p95 | 20.48 ms | 20.82 ms |
| Resize layout p95 | 1.42 ms | 1.44 ms |
| Resize render CPU p95 | 13.28 ms | 14.46 ms |
| Composer render CPU p95 | 14.87 ms | 15.15 ms |

Both retained five timeline rows and three images during scrolling. This single
pair shows comparable cached scrolling cost, not zero overhead or a compositor
frame-rate guarantee. Xvfb reports no DRI3 acceleration. PDF download latency,
concurrent parsing workload and live native Vulkan timing are not established by
this benchmark.

Installed SHA-256 matched the optimized build:
`b3e35da10d00fb9df10f8c2f047e1b44506b88409ea5cc2bef4b23f2c8552864`.
The old window closed normally in 0.12 seconds. The normal desktop entry reopened
without `--offline` and reached Connected, with one Heya process, no idle audio
worker and no idle PDF renderer. The owner opened the affected PDF and confirmed
that its preview is now sharp. This establishes that particular live document's
preview path; it does not establish availability/quality of every archived file.
A final read-only metadata check confirmed that the affected preview changed
from a 51×72 embedded thumbnail to a cached 768×320 strip. Other older documents
remain eligible for upgrade when viewed; no bulk download was triggered.

## Shared emoji picker and outgoing read ranges — 16 October 2026

Pinned ZapFast visual reference: `src/ui/picker.rs::reaction_picker`,
`emoji_grid`, `category_tabs`, `rows_for` and `place_picker`. Its reaction picker
uses a 420×400 inner area, 10px frame margins, 40px rows/cells, 24px emoji,
12.5px section headings, 13px title and 12px one-line message summary. The footer
uses 24–36px category cells, a 22% accent highlight and 2px selected underline.
Heya implements that structure in one conditional Slint picker shared by message
reactions and the composer. Forwarding keeps its separate destination dialog.
Search matches names, shortcodes and literal emoji. Category tabs scroll the
virtualized grid, and arrows/Enter select from it. Existing own-reaction toggling
still removes a reaction. Composer insertion uses the native editor's UTF-8
selection offsets and normal draft-edit/autosave path; choosing never sends the
draft, repeated choices keep the picker open, and closing restores composer focus.
Recent/frequent choices are bounded to 40 and currently last only for this app
session. They stay in their opening order while the picker remains open.

Only visible emoji rows have controls. Headless checks measured 79 interactive
targets for the visible picker, with an upper bound of 160 during category jumps;
there is no control for every database emoji. Category jumps reuse the existing
row model when there is no search to clear. Closing destroys the picker, and
opening it leaves timeline geometry unchanged. Tests cover Unicode insertion,
reversed selections, invalid UTF-8 offsets, empty searches, keyboard insertion,
reaction removal, composer button activation and containment at 1200×900,
620×700 and 520×480. The resize test exposed an out-of-date anchor extending the
popup below a resized window; placement now clamps the entire popup on both axes.

The separate unread bug came from deduplicating against the local badge: outgoing
messages do not increment it, so a newer outgoing position was skipped when it
was already zero. Confirmed sends now advance the existing private read queue via
ACK/receipt or the sent-message echo. No extra view tracking, worker, timer,
schema or SDK patch was added. Queued/failed sends do not create remote positions.
Server ACK time corrects the time used for offline-queued sends. Read ranges carry
the original message identity/from-me flag, including group author metadata; the
incoming app-state adapter preserves that boundary when the SDK re-fetches the
mutation. This prevents rounding an outgoing read over newer incoming rows in the
same second. Repeated acknowledgments/revisits deduplicate, and stale completion,
manual unread marks, deletion and restart remain covered by synthetic tests.
Old outgoing history does not trigger a broadcast while importing.

Validation: 131 application/service/UI tests and one protocol-storage integration
passed, with the deliberate keyring test still opt-in. Synthetic tests do not
establish cross-client behavior. The owner subsequently sent a message manually
and confirmed that the chat stayed read in the other WhatsApp client. Artifacts:
`.prep/validation/outgoing-read-20261016/`.

Native synthetic captures cover the reaction picker, light/dark composer picker,
2× scale and a 520×480 window. A proof capture exposed that a composer picker
was incorrectly treated as a message-target dialog during background snapshots.
It now survives conversation updates; the toolkit regression checks both the
open state and the unchanged draft. Closing/changing chats still dismisses it.
The final targeted UI test and all-target Clippy with warnings denied passed.

Early benchmarks labelled “picker open” are discarded: startup changed-chat
handling closed the popup before the first frame. The benchmark now opens it
after idle and asserts its open state at each subsequent phase. Independent
synthetic X11 captures also verified it remained visible through update and
resize/scroll phases. Those captures never access the owner's desktop.

Final optimized document-workload benchmark, private Xvfb/OpenGL, 10,000 loaded
messages and 500 chats (milliseconds, p95):

| Measurement | Picker closed | Picker open, category jumps |
| --- | ---: | ---: |
| Scroll render CPU | 16.50 | 16.63 |
| Scroll frame interval | 21.57 | 21.45 |
| Scroll timeline layout | 0.15 | 0.14 |
| Resize render CPU | 14.10 | 15.64 |
| Resize timeline layout | 1.43 | 1.76 |
| Composer render CPU | 15.23 | 18.03 |

Both scrolling cases retained five timeline rows and three thumbnails. These
single-run comparisons indicate similar steady scrolling cost, not identical
performance. An additional capture-assisted open-picker run measured 17.64 ms
scroll render p95. Cold popup/category changes produced a worst timer lapse of
723 ms in the final run, and early resize render p95 was 28.52 ms. These pauses
are recorded rather than hidden by the steady-state result; the measurements do
not isolate model construction, text/font initialization and software rendering
costs. No prewarming worker or permanent thread was added. Native GPU/compositor
behavior remains unmeasured: this private X server reports unavailable DRI3.
Logs: `closed-benchmark.log`, `picker-final-benchmark.log` (capture-assisted), and
`picker-verified-benchmark.log` (final assertions, no captures), under the artifact
directory above. The final release also passed all-target Clippy with warnings
denied.

Installed SHA-256 matched the final optimized binary:
`d53989a9052373af17ea42254ee3427e148e6656c9a4f18cc934e84df510460c`.
The normal desktop entry reopened without `--offline` or a forced renderer and
reached Connected, with one Heya process. No existing Heya process was present
at installation, so no additional window close was necessary. The live check
did not send any message automatically.

## Automatic history paging — 16 October 2026

Pinned ZapFast `src/ui/conversation.rs` checks an 80px top threshold or a
transcript that fits the viewport, and `src/app.rs::load_older` guards a pending
page and preserves a message anchor. Heya now uses those interaction rules for
the existing encrypted-archive pages. The 38px button band and its “Load older
messages” label are removed. The current identity-based virtual timeline keeps
the viewed message in place as 100 earlier messages are prepended.

Paging runs from the existing layout/viewport events, with no new polling
timer, worker or UI control. A single pending request is released only by its
matching chat/request reply; unrelated snapshots cannot duplicate it. Live
requests also carry the navigation revision. Reopening a chat cancels old UI
requests without letting their late completions release newer work. Short
pages fill automatically, including offline. Hidden/minimized conversations,
navigation in progress and dialogs suppress new requests. At the archive's end
there are no further requests. Heya still relies on imported history; this
change does not add phone-history requests.

Page queries/decryption remain on the archive worker. The worker commits an
expanded page size only after a successful query. A failed query or full command
queue blocks repeated layout retries until the reader moves away from the top
and returns, or reopens the chat. Existing archive errors use the notice banner.

Validation: 132 application/service/UI tests and one protocol-storage integration
test passed; the real-keyring test remains opt-in. The new toolkit checks cover
the 80px trigger, one request during repeated snapshots, content anchoring across
prepend, bounded row instances, stale replies after reopening the same chat,
minimized suppression, queue rejection/retry, offline viewport filling and
exhaustion. The read-visibility regression now also verifies that reaching older
history queues a page without sending a read receipt. A disposable SQLCipher
fixture exercises stale navigation, a real failing page query, successful retry
at the original limit, and exhaustion. Strict all-target Clippy and the
media-disabled check passed. Logs are under
`.prep/validation/auto-history-20261016/`.

The optimized native wheel check ran 100 upward notches on a private X server
in the synthetic Timeline lab. The old build stopped at message ID 9921; the new
build reached ID 9775 through automatic prepends. These are synthetic local IDs,
not account identifiers. Both kept fewer than 30 row controls, and no pair of
samples less than 100ms apart jumped by 60 message IDs or more. This coarse
wheel trace is separate from the precise content-anchor toolkit regression;
it does not prove every compositor frame. Before/after native captures also
confirm the button band is gone. Startup-chat trace entries before the queued
demo selection were excluded from the comparison.

One sequential before/after optimized cached-document benchmark used the same
private Xvfb/OpenGL setup, 500 chats and 10,000 loaded messages (p95, ms):

| Measurement | Before | Automatic paging |
| --- | ---: | ---: |
| Scroll render CPU | 15.24 | 16.71 |
| Scroll frame interval | 19.29 | 20.99 |
| Scroll timeline layout | 0.14 | 0.15 |
| Resize render CPU | 12.77 | 14.51 |
| Resize timeline layout | 1.45 | 1.88 |
| Composer render CPU | 13.94 | 15.27 |

Scrolling retained five timeline rows and three thumbnails in both cases.
Removing the 38px band expands the visible transcript, so the geometry is no
longer identical; resize measured 428 rows before and 463 after. This pair
establishes bounded work and records the additional measured cost, without
claiming identical performance or attributing the difference to one cause.
Xvfb reports no DRI3 acceleration; native Vulkan/compositor behavior and account
history availability are not established by these checks. No automated account
message or phone-history request was sent. Logs and synthetic captures are in
the validation directory above.

Installed SHA-256 matched the verified optimized build:
`ccd47485334e74200f69e369d7c8841881a470e7f6c8cf6fbcf491c5d183d657`.
No Heya process was present when installing, so no window close was needed.
The normal desktop entry reopened with its saved session, without `--offline`
or a forced renderer, and reached Connected with one Heya process.


## Explicit attachment Download — 16 October 2026

The existing hovered-bubble menu now connects Download to a native Linux Save As
picker and independent application-owned transfer service. The supplied Windows
desktop audit in [desktop-media-audit.md](desktop-media-audit.md) guided the picker,
cancellation and verified-completion behavior. The pinned SDK provides streaming
authenticated downloads without another patch; no WebView is introduced.

One conditional window-level 360×104px card shows the selected filename, status,
progress and Cancel/Retry/Dismiss. It clamps within narrow windows and does not
change transcript/composer geometry. The existing message-row shape is unchanged.
Progress is coalesced to at most ten updates per second; no progress polling timer,
per-message download control or dedicated idle transfer thread is added. The
offscreen framebuffer caught a centered progress-fill rectangle; it now starts
at the track's left edge, with an explicit half-width toolkit assertion.

Validation: the full suite passed 146 application/service/UI tests and one
protocol-storage integration test, with the real-keyring test still opt-in. After
the progress-fill correction, the full toolkit workflow test and strict all-target
Clippy passed again. The media-disabled configuration check passed. A 1,000-update
regression leaves timeline layout counters, row model and geometry unchanged;
containment is checked at 1200×800, 800×600 and 520×360. SDK/file/service tests cover
streaming, integrity failure, retries, cancellation, offline cached originals,
destination changes and shutdown source release. See [downloads](downloads.md).

The installed KDE file chooser was also exercised through the actual Heya demo
menu, with separate private X server/D-Bus/XDG environments for Save and Cancel.
Save produced the exact synthetic original without a partial file; Cancel
produced no output and displayed Retry. A private synthetic D-Bus service tests
early responses and Request.Close separately. Native demo/no-session window
close/reopen checks exited in 23–46 ms and found no idle audio thread. These
checks do not access an account or establish live CDN/compositor compatibility.

Artifacts and logs: `.prep/validation/downloads-20261016/`.

Sequential hidden-panel/progress-panel runs used the same final release binary,
500 chats and 10,000 loaded cached-document messages on private Xvfb/OpenGL.
The progress run published synthetic status approximately ten times per second;
it performed no network or file transfer. Final p95 measurements (ms):

| Measurement | Hidden | Progress updating |
| --- | ---: | ---: |
| Scroll render CPU | 15.43 | 16.16 |
| Scroll frame interval | 19.92 | 20.78 |
| Scroll timeline layout | 0.13 | 0.15 |
| Resize render CPU | 13.34 | 14.32 |
| Resize timeline layout | 1.65 | 1.62 |
| Composer render CPU | 14.03 | 16.28 |
| Composer frame interval | 32.31 | 46.64 |

Both scroll runs retained five timeline rows and three document thumbnails. The
progress panel therefore keeps timeline residency bounded, with a measured render
cost in these single-run samples. The composer result is recorded rather than
claiming identical speed. An earlier pair before the fill-alignment correction
measured scroll render p95 at 14.37/14.60 ms and composer render at 13.63/13.81 ms,
which also illustrates run-to-run variation. Xvfb reports unavailable DRI3; these
are application/render CPU measurements without native GPU/compositor or live CDN
behavior. Cold initialization and a scroll-phase transition produced timer lapses
up to 211 ms in the final progress run; these are not steady-frame guarantees.
Logs are `closed-benchmark.log` and `progress-benchmark.log`; `progress.png`
confirms the final panel and left-aligned progress fill.

Final optimized build SHA-256:
`2d56879ac86dbb0e950c9ec42e35729ebee4eeeeac7f062f428dd533e715da1a`.

Installation verified the installed binary has the same hash. The previous normal
window closed in 0.12 seconds; the desktop entry reopened without `--offline` or
a renderer override and reached Connected. No automated account transfer or
message send was performed. The owner explicitly tested a real received attachment
and confirmed that it saved and opened correctly. No private filename, source
identifier, destination or content is recorded here.


### Download success-panel dismissal correction

The first download build left Saved visible until manually dismissed. It now
clears automatically after three seconds. A single async deadline exists only
after success; there is no polling loop, extra thread or timeline change. Starting
another transfer or dismissing the panel clears the old deadline. Failed and
cancelled transfers retain Retry.

Nine focused download/SDK tests and strict all-target Clippy passed. The new
regression verifies that Saved remains briefly visible, that the old deadline
cannot hide a subsequent pending download, that a failure remains visible beyond
three seconds, and that a later successful save clears without changing its file.
The optimized private KDE Save As flow also passed: the synthetic original saved
correctly and, after 3.5 seconds, the panel region matched its original background
exactly. The capture was visually checked. No account transfer was automated.
Artifacts: `.prep/validation/download-dismiss-20261016/`. The previous render
measurements above remain measurements of the preceding build; this correction
changes only completion-state lifetime, with no new rendering controls.

Installed build SHA-256 matched the verified release:
`adbbdf82fbacc55491d17372aa5604c54d454d2940e94c1e5d01d3985ad88dd1`.
The previous window closed normally in 0.12 seconds; the normal desktop entry
reopened with the saved session and reached Connected.

### Outgoing photos/files and transfer lifetimes — 16 October

The composer + opens Photos/Documents, following the supplied desktop shell's
attachment colors and an independently implemented native Slint review. The
Windows inspection establishes its registered-file/stream cleanup boundary; it
does not establish the web editor's exact layout or compression policy. See
[the desktop audit](desktop-media-audit.md) and [attachment behavior](attachments.md).
The new `photo.svg` uses the existing Lucide outline vocabulary/ISC notice.

The review shows the captured recipient, a bounded photo preview or file
name/size, per-file captions, selection/removal and explicit Send. After Send a
single nonmodal status card replaces the review. Progress retains model/texture
identity and does not replace chat rows. Captions retain Unicode/multiline text
and follow the caret when their viewport is full. Uploads use the SDK public API;
no additional protocol-library patch was needed.

Final validation:

- 155 application/service/UI tests plus the encrypted protocol-storage integration
  passed; the real-keyring check remains opt-in. Later focused SDK and Slint
  workflow checks passed after the size-limit/caret refinements. Strict all-target
  Clippy passed, as did the no-default-features build check.
- Synthetic upload tests verify original photo bytes/orientation, quote/envelope
  metadata, authenticated 32 MiB streaming, independent resume offsets, immutable
  ciphertext, cancellation, batch retry, stable outbox IDs, unchanged text drafts,
  read-only rejection and active-upload service closure. A real finite executor
  test waits for blocking work, releases Client ownership on cancellation, joins
  workers and confirms no named upload thread survives completion. No account
  upload was automated.
- The actual KDE OpenFile portal selected a synthetic PNG and text document on
  private Xvfb/D-Bus sessions. Review captures at 1200×800 and 520×480 were visually
  checked, and no preparation/upload thread remained idle after preparation.
  Normal close exited in 76/72 ms. Four additional native demo/no-session startup
  close/reopens exited in 47–77 ms without idle media threads. Toolkit controls
  were also checked at 800×600 and 520×360. These are not live-network shutdown
  or real keyring/compositor tests.

An optimized, sequential OpenGL/Xvfb comparison used 500 synthetic chats,
10,000 messages, encrypted cached document thumbnails, and then the same workload
with ten staged review images and upload progress at 10 Hz:

| Phase | No upload status: render CPU p95 | Upload status: render CPU p95 |
| --- | ---: | ---: |
| Changing snapshots | 12.20 ms | 11.83 ms |
| Scroll, 100 messages | 15.42 ms | 15.81 ms |
| Scroll, 10,000 messages | 15.65 ms | 14.93 ms |
| Resize, 10,000 messages | 14.54 ms | 13.51 ms |
| Composer, 10,000 messages | 15.73 ms | 15.29 ms |

These single runs show similar warm render costs, not a measured speedup. The
active status produces expected redraws during otherwise unchanged snapshots
(61 frames, p95 13.52 ms versus zero redraws when closed). Scroll frame-interval
p95 remained about 20 ms; composer frame intervals varied, 32.46 versus 47.09 ms,
while timer-lateness p95 was 2.20 versus 1.60 ms. These figures do not measure
input-to-display latency. Both runs had an approximately 2.3-second cold initial
render in Xvfb; it is not included in the warm comparisons. A 1,000-status-update
Slint regression separately confirms unchanged preview/model identity and no
extra timeline layout from progress alone. Real GPU/compositor animations,
network throughput, native-account transfers and total RAM are not established
by these measurements.

Artifacts: `.prep/validation/attachments-20261016/` (native captures, complete
benchmark/test logs and install/reopen evidence). The verified optimized binary
SHA-256 is `ad52db9f03120c81676542cb08fd9e439b88df41a9e187629c3e5e01fbd4924e`.

The matching binary was installed and the normal desktop entry reopened with
one Heya process and the saved session at Connected. No account send was
performed by automation; manual photo/document arrival/opening is pending the
owner's check.

### False cancellation after upload completion — 16 October

The owner reported an Upload cancelled banner while testing the first attachment
build. A worker regression reproduced it: the worker's CancelOnDrop guard fired
on normal return, cancelling the same token the coordinator checked before
queueing. Successful HTTP completion could therefore be mistaken for a user
cancellation, leaving the selection retained without an outgoing message.

The guard is now armed only after worker creation and disarmed after a completed
result and worker join, including ordinary error returns. Abandonment or explicit
cancellation still stops the SDK future. The synthetic service uploader now runs
through the actual worker rather than bypassing it, so batch/retry/archive tests
exercise this completion-to-queue boundary. The worker tests also check normal
success/error cancellation state, active-future abandonment, session-owner
release, blocking-work retirement, and the exact threads owned by that test
rather than unrelated concurrent tests' thread names.

The regression failed before the fix. The corrected full suite passed 157
application/service/UI tests plus the encrypted protocol-storage integration;
the real-keyring test remains opt-in. Artifacts are in
`.prep/validation/upload-cancel-20261016/`. This fixes a local lifecycle error;
manual account attachment delivery still needs confirmation. The preceding
render measurements describe the preceding build; no UI control/layout was
changed for this correction.

The corrected optimized binary installed with matching SHA-256
`e24b68d0aa54585f7935f04756a478b50025f0282f6d0568ec353f2f70f333fd`.
Strict all-target Clippy passed. Four native offscreen close/reopens exited in
40–74 ms without idle media threads. The owner's old window closed normally in
0.12 seconds; one updated process reopened at Connected. No account send was
automated, and the owner was asked to reselect/send the attachment manually.

On 17 October, the owner confirmed: “It sends and arrives correctly.” This
establishes manual attachment delivery after the cancellation fix, including
appearance in Heya and arrival in another WhatsApp client. The attachment's type
and size were not specified; no broader media, batch, interruption or file-opening
claim follows from this check. No automated account message was sent.


### Sending voice messages — 17 October 2026

The pinned ZapFast `conversation.rs::recording_strip` (line 7461) replaces the
one-line composer with discard, recording light, timer, recent waveform and
send. Heya uses that visual arrangement and existing Slint icon/button styling,
with pause/resume and an explicit Stop → Review → Send step. The controls are
one composer component; one path draws 64 bars, and progress is coalesced at
10 Hz. No timeline row owns a recorder or receives capture-progress updates.

The native offscreen inspection of the initial in-process design found three
GStreamer pool/source threads left in the parent after capture stopped. Capture
and review playback were consequently moved to short-lived instances of the same
binary. Bounded binary pipes carry controls/progress/Ogg bytes; helpers initialize
neither Slint nor account services. Completion waits for process exit, and parent
pipe closure stops a helper even after a parent crash. No plaintext media file,
new permanent thread, daemon, protocol patch or dependency was added.

The synthetic tests exercise actual GStreamer tone → Ogg/Opus capture, pause,
finalization, preview through a silent sink, SDK Audio encryption/decryption,
PTT metadata/quote/waveform/sidecar construction, encrypted outgoing audio cache,
explicit handoff, failed upload/retry, preserved text draft, disconnected review,
refused handoff, cancellation, owner drop and parent-pipe EOF. Malformed IPC
lengths are rejected before media allocation. The toolkit workflow checks one
recording strip at four window sizes and verifies that 1,000 progress updates
preserve the timeline model/height/position and draft. The full suite passed
164 tests plus the encrypted protocol-storage integration; the desktop-keyring
check and internal subprocess fixture are opt-in/ignored in the ordinary run.
No automated check records the owner's microphone or sends an account message.

Artifacts are in `.prep/validation/voice-20261017/`. Real microphone permissions,
default input/output routing and recipient voice-note playback remain for the
owner's deliberate check. See [voice messages](voice-messages.md) for behavior,
limits and teardown details.


The follow-up adds an explicit checking-input state. Only an enumerated, eligible
GStreamer input can create a capture pipeline; output-monitor sources are excluded,
and there is no automatic-source silent/generated fallback. The ordinary composer
remains visible during the check; only actual sampled audio activates the recording
strip. Device absence/monitor filtering and this UI state have synthetic coverage.
The owner reports no microphone attached; initial read-only PulseAudio/GStreamer
inspection found only an output monitor/no eligible input. This is distinct from the
demo's explicitly synthetic tone.

Settings now persist microphone/speaker choices and separate recording/playback
volumes through a shared preference snapshot. Operations resolve the chosen route
when starting; unplugged selections remain visible as unavailable. A finite helper
lists devices on demand; settings callbacks update only the two device rows, keeping
unrelated popups/sliders intact. Save As now starts in the configured Downloads
folder. Defaults use the platform user-directory API and append Heya once. Native
folder selection uses the existing portal with directory/multiple/current-folder
contract tests. Folder cancellation preserves the previous setting. Configuration
and directory filesystem jobs use finite joined workers, and closing the window
cancels/joins pending settings operations.


Default-feature Clippy and media-disabled Clippy both pass with warnings denied.
The last full suite passed 164 application/UI tests and the encrypted-storage
integration. The old immediate `/proc` assertion in an attachment cleanup test
was racy under concurrent load: it now observes retirement of the exact joined
OS tasks with a bounded wait, rather than mistaking a transient exit entry for
an idle worker. No upload completion/cancellation behavior changed in this test
correction. Ordinary completion still joins the worker and preserves the caller's
cancellation reason.


The optimized native offscreen run passed capture, pause/resume, stop/review,
preview/stop/discard, narrow layout, opening Settings during capture, settings
search after cancellation, and normal close during capture. It found no new parent
threads after Stop or preview teardown. Normal close during synthetic capture
exited in 87 ms; after killing a separate synthetic parent, its capture helper
exited and was reaped within 32 ms. The active helper measured 27.8 MiB RSS /
16.3 MiB PSS; those mappings disappear with helper exit. A late completion focus
bug was reproduced and corrected with a shared visible-composer guard; the full
UI workflow regression passed again, followed by strict all-target Clippy.
Actual read-only device probes found zero eligible inputs and one output;
`probe-input` returned NoInput without creating a recording pipeline.

Final optimized Xvfb/Skia OpenGL comparisons used 500 chats, 10,000 messages and
a bounded worker decoding synthetic photos from a temporary encrypted archive.
The recording variant ran real varying-tone Ogg/Opus encoding with 10 Hz progress.
No compiler/test jobs ran alongside either benchmark. Render CPU p95:

| Phase | Baseline | Recording |
| --- | ---: | ---: |
| Changing snapshots | 11.68 ms | 11.77 ms |
| Scroll 100 | 10.17 ms | 14.60 ms |
| Scroll 10,000 | 10.86 ms | 13.94 ms |
| Resize 10,000 | 11.70 ms | 12.30 ms |

Recording adds approximately 3.1 ms to the 10,000-message scroll p95 in this
software-backed OpenGL run. Timeline layout p95 remains 0.15/0.16 ms, with no
new rows measured during that scroll phase; 3–4 inline photos are resident.
Cold first-frame work is excluded from these steady phases. The final
composer phase is not a typing comparison because recording replaces the editor.
These are application/render measurements, not a compositor animation or a real
microphone/recipient compatibility claim. Real microphone and recipient playback
remain unverified because no microphone is currently attached. No live account
message or real recording was automated.


The matching optimized binary was installed and the normal desktop entry reopened
at Connected, with one ordinary Heya process and no idle media helper. SHA-256:
`096e351df62bf95fef705444f8e62ce28ad65c4985e042c1882e0b88f83e254b`.
No live message or real microphone recording was automated.

## Missing microphone feedback — 17 October 2026

Clicking the microphone with no available input now opens a single, conditional
window-level “Microphone not found” dialog. It explains connection, input
selection and permissions, with an “OK, got it” button. Enter and Escape also
dismiss it; keyboard input cannot reach the composer underneath. Dismissal
allows a fresh check on the next click. The microphone icon now uses a dark
foreground on its pale button, instead of appearing disabled with white ink.

The recorder preserves the typed missing-input failure through its status
snapshot. Completion still waits for helper exit/reap before reporting the
error. No recording pipeline, continuous device monitor, per-message dialog or
idle media helper is introduced.

Validation: 165 application tests plus the encrypted-storage integration test
passed; the comprehensive UI workflow passed again after the contrast change.
Strict Clippy checks passed with and without the GStreamer feature. Optimized
native checks used `scripts/ui-offscreen.sh`, private Xvfb and Skia/OpenGL, with
a synthetic missing input. They exercised helper IPC, repeated clicks, typing
isolation, OK/Enter/Escape, 1200×800 and 520×480 layouts, and helper cleanup.
Screenshots were inspected. Polling times in the harness include screenshot
and pixel-scan overhead and are not UI latency benchmarks. The real device
metadata probe also returned NoInput; it did not capture sound. No live account
message or real recording was automated.

Evidence: `.prep/validation/mic-error-20261017/`. Optimized binary SHA-256:
`5fb0bfebf96093a760963d260a6545e8464809c5b08c132b6b3d9a562dd978b6`.
Installed and reopened the normal saved session at Connected, with one Heya
instance and zero media helpers.


## ZapFast right-click controls and composer focus — 17 October 2026

The pinned ZapFast `conversation.rs::context_menu`, `quick_reactions` and
`selection_bar`, shared `widgets.rs` menu frame/items/separators, app eligibility
rules and message-info/dialog views were inspected. Heya now has the integrated
right-click reaction/menu card: 34px reaction controls with 2px gaps, 28px action
rows with 6px spacing, 13.5px text and 16px icons. Defaults/usage preferences and
an existing own reaction determine quick choices. Hover Forward keeps its double
arrow; the context-menu Forward uses the reference's single arrow. The chevron
retains its previously implemented native WhatsApp-style menu.

The actions include reply/react/forward, bounded selection, complete text copy,
eligible own-text edit, sender revocation, deletion for me, message info and exact
wire-ID copy. Edit/delete permissions are rechecked behind the UI; edits and
revocation share the existing durable outbox and wait for ACK/echo evidence.
Local deletion has replay-safe tombstones and idempotent linked-device sync.
Info uses actual supplied receipt evidence and stops its observation on close.
Unknown-chat sync events cannot invent sidebar entries. These are independent
Heya implementations; no new whatsapp-rust patch was needed. See
[message-controls.md](message-controls.md) for boundaries and SDK limitations.

Native testing caught child media controls consuming right clicks. One window
input handler now resolves the existing exact cached timeline bounds before
Slint dispatch. There is no per-message polling or extra thread. The composer
focus outline takes its radius directly from the rounded pill. Inactive
selection callbacks/decorations and the unfocused outline are absent from the
scene. Selection does not rebuild the timeline or change measured row widths.

Validation: 177 application tests plus the encrypted-storage integration passed,
with two deliberately ignored environment-dependent tests. Strict all-target
Clippy passed with and without GStreamer; formatting passed. Optimized native
checks ran only on `scripts/ui-offscreen.sh`'s private Xvfb/Skia OpenGL display,
using synthetic data. Text, previews, photo/audio/document fixtures, edit,
receipt info, selection, deletion keyboard handling, retained drafts, 1200x800
and 520x480 layouts and rounded composer pixels passed. Screenshots were
inspected; right-clicking audio left no playback helper. Screenshot polling
allows cold first-frame work and is not a latency benchmark.

Earlier new-UI render variants showed substantially worse offscreen timings;
they were not installed. After removing inactive drawing/callbacks, two optimized
runs gave the following render-CPU p95 figures. The comparison build is the
previously installed optimized binary, using the same 500-chat/10,000-message
synthetic photo workload, with no compiler alongside either measurement.

| Phase | Previous build, comparison run 2 | Final optimized run 1 | Final optimized run 2 |
| --- | ---: | ---: | ---: |
| Changing snapshots | 12.00 ms | 12.02 ms | 12.06 ms |
| Scroll 100 | 10.66 ms | 14.15 ms | 11.95 ms |
| Scroll 10,000 | 11.06 ms | 11.08 ms | 12.01 ms |
| Resize 10,000 | 16.45 ms | 10.86 ms | 11.30 ms |

Final 10,000-message scroll layout p95 was 0.16ms, with zero rows remeasured;
3–4 inline photos/rows were resident. The runs also show environmental and cold
frame variability, so these do not establish compositor animation, hardware
Vulkan performance, or live server/account compatibility. Live edit/delete
interoperability still needs deliberate manual validation. No owner-account
message/edit/delete was automated.

Evidence: `.prep/validation/context-menu-20261017/`. Final optimized SHA-256:
`607173112db9e1326fca34e1f68db761074b1819f00e81bf7dbe285600aa66dd`.

Installed that exact build and reopened the normal desktop entry to Connected.
The previous window closed normally in 0.22s. One ordinary Heya instance and
zero media helpers were observed; neither `--offline` nor `--demo` was used.


## Color emoji in chat text and reactions — 17 October 2026

The pinned ZapFast `src/markup.rs::layout` uses 2.4× body size for one to three
emoji-only graphemes; `src/emoji.rs` paints platform/bundled color-font images
for egui. Heya retains its existing Slint text controls and native Skia glyph
rendering. One shared boolean display rule selects the enlarged body size;
natural-size probes, wrapped measurement and drawing use the same size. Text
is unchanged, including ZWJ, skin-tone, flag and keycap sequences.

A localized Parley 0.11.1 patch corrects emoji font priority ahead of monochrome
symbol coverage. Plain digits/#/* retain their text font, while keycaps prefer
emoji; VS15 preserves text presentation. This also colors mixed body text,
reaction summaries, the sidebar and composer through the normal text shaper.
The existing virtual timeline/cache stays in place: no new per-message control,
image overlay, timer, worker or cache-size setting. Source base, exact patch and
provenance are in `docs/color-emoji.md`, `patches/README.md` and
`prep/upstreams.json`. No upstream submission has been made.

Validation: 179 application tests and one encrypted-storage integration test
passed; two existing environment-dependent tests remain ignored. Strict
all-target Clippy passed with and without GStreamer, and formatting passed.
Pure tests exercise font routing and complete emoji sequences; shared toolkit
checks exercise unchanged text, light/dark themes, repeated narrow/wide resizing,
and timestamp/body containment. Private-Xvfb native Skia OpenGL pixel checks
found color in large single/triple emoji, mixed text, reaction badges and four
ordinary-sized emoji. Native selection/copy preserved the original single and
mixed emoji text. No media helper was created. All data is synthetic.

Optimized runs had no concurrent compiler. The initial old/new comparison gave
14.07/15.51ms render p95 for scrolling 10,000 messages, so a back-to-back check
was run to investigate the difference:

| Phase | Previous build, paired | Color-emoji build, paired |
| --- | ---: | ---: |
| Changing snapshots | 16.14ms | 17.16ms |
| Scroll 10,000 | 14.46ms | 14.14ms |
| Resize 10,000 | 18.34ms | 17.61ms |
| Composer 10,000 | 24.75ms | 23.04ms |

Paired 10,000-message scroll layout p95 was 0.27ms, with zero remeasured rows.
A separate 10,000-message workload containing enlarged emoji, mixed text,
flags/ZWJ/keycaps and reaction counts gave 18.08ms scroll render p95 and 0.31ms
layout p95. These are virtual-display renderer/application timings, with visible
environmental variation, not a hardware/compositor or end-to-end 60Hz guarantee.
Windows and macOS remain unverified. Evidence is in
`.prep/validation/color-emoji-20261017/`.

Timing-build SHA256: `f8f100b73358a80cc7d114b9197c8dada2ec9a7e5f0a0c49b2c559d31fbb0d37`.
The final installation build changes only the synthetic emoji fixture's plain
caption; native visual/copy/resize checks were repeated on that exact binary.
Installed SHA256: `87b285a0b618f5232535a98054c2e4fbd497c7b4a972ba2c54c96a91c96434d1`.
The previous window closed normally in 0.22s; the saved session reopened to
Connected, with one ordinary instance, neither demo nor offline mode, and no
media helper. No live account message was automated.


## Phone-history requests and application-path measurements (17 October 2026)

Heya uses the pinned SDK's public `Client::fetch_message_history` API, with 50
messages per request. The oldest confirmed wire key, direction and Unix-seconds
timestamp come from the encrypted archive; unconfirmed sends cannot anchor a
request. The misleading `timestamp_ms` parameter name is checked against
[WhatsMeow's primary request builder](https://github.com/tulir/whatsmeow/blob/main/send.go),
which writes Unix seconds to that field. ZapFast's pinned worker also requests
50 messages. No SDK fork is required for this feature.

The capture hook and SDK history event share compressed bytes. Their SHA-256
identity associates captured data with every supplied request ID, including
identical replies to different requests. SQLCipher schema 14 records those
associations and per-conversation terminal history flags. Requests become
answered only after the corresponding chunk is imported. Replayed chunks and
PN/LID merges retain that evidence. The app waits through the existing messaging
worker's select loop; there is no extra idle history service/thread. Requests
cancel on navigation, disconnect or close. A silent/empty response backs off;
reported exhaustion persists across reopening. Synthetic tests establish those
failure/lifecycle cases. The owner confirmed older messages loading through the
updated live phone-history path on 17 October 2026. Other phone/network failure
conditions remain manual validation gates.

`HEYA_BENCHMARK_ARCHIVE=1` adds a disposable encrypted archive behind the real
messaging worker and coalesced UI callback. Unlike the prebuilt-view benchmark,
it measures queue/lock wait, SQLCipher query/projection, snapshot publication,
UI application, and ingest/command enqueue through the next rendered frame.
It includes 500 chats, paging to 10,000 loaded variable-height Unicode messages,
single arrivals, unrelated updates, twenty-message bursts, wheel scrolling and
navigation. Issued/applied/rendered counts accompany the latency samples so
coalescing cannot masquerade as all updates being rendered. Values are numeric
and synthetic; no account content enters a fixture or log. The probe is enabled
only on the disposable benchmark archive. Renderer completion is application
submission/CPU timing, not GPU completion or compositor presentation; protocol
transport, Signal cryptography, physical input and real phone latency remain
outside this benchmark.

The first full-path run exposed costs absent from the renderer benchmark:
incoming twenty-message bursts at 10,000 loaded rows had 582.31 ms p95 from
ingest start to the next render (7 observed generations), while render CPU p95
was 10.07 ms. Unrelated traffic had 117.64 ms p95 query/projection and 254.34 ms
p95 ingest-to-render. These are recorded in `archive-initial.log` under
`.prep/validation/history-performance-20261017/`; cold first-frame render work
was 3697.69 ms on the private CPU-rendered display, reported separately.

Query-plan inspection using the same compiled SQLCipher engine confirmed that
edit reconciliation was driven by every message in the chat even with no
mutations. It now drives indexed message lookups from the sparse mutation table.
Schema 14 adds covering indexes for visible timeline lookups and both active and
expired expiry ranges. Control cleanup avoids materializing its message subquery
when no retained operation text needs clearing. These are query/index changes;
no new view cache, media worker or cache-size tuning is involved. The final
full-path rerun below is the evidence for their effect, rather than the
in-memory query-plan harness's timings.

Final optimized rerun, same synthetic input policy and private Xvfb/Skia OpenGL,
with no compiler alongside either run:

| Application-path measurement | Before p95 | After p95 |
| --- | ---: | ---: |
| Incoming bursts: ingest start → render (10,000 loaded) | 582.31 ms | 175.98 ms |
| Incoming burst commit | 437.80 ms | 51.31 ms |
| Unrelated traffic: ingest start → render (10,000 loaded) | 254.34 ms | 154.25 ms |
| Unrelated traffic: query/projection | 117.64 ms | 87.28 ms |
| Navigation command → render | 84.69 ms | 48.33 ms |

The incoming phase processed 140 versus 700 messages, observing 7 versus 35
rendered generations. The producer awaits archive commit, so the improved
throughput is visible rather than represented as an identical fixed arrival
rate. Paging through 99 production worker commands took 7291.30 versus 4729.01 ms.
These are one before/after pair, not a sustained latency guarantee. The final
cold first-frame CPU render sample was 1766.09 ms; warm samples do not describe
startup or the owner's native Vulkan/Wayland driver. Full conversation
projection on unrelated traffic remains a measured optimization target.

The separate paired photo-render workload remained bounded: 10,000-message
scroll render CPU p95 was 10.28 versus 11.01 ms; final layout p95 was 0.13 ms,
with zero rows remeasured. Both had 187 observed frames. Native offscreen menu
capture confirms the new action fits without clipping. Final validation passed
192 application tests and the encrypted-storage integration, with two existing
environment-dependent tests skipped, plus strict default/no-default Clippy.
Artifacts and machine-readable samples are under the directory above.

The verified release was installed and reopened normally, reaching Connected.
The owner then confirmed “Older messages loaded” in response to the live
phone-history check. No automatic chat-message send was part of validation.
