# Received locations

Heya displays received fixed locations as map cards with a pin, optional place
name/address and website domain. Clicking the map opens its coordinates in
Google Maps through the normal desktop browser launcher. The card's context
menu copies the map link, coordinates or address, and opens the supplied place
website when it is a valid HTTP(S) URL. Reply, reaction, star and the existing
message actions remain available. This does not add location sending or live
location tracking.

The existing whatsapp-rust API already exposes the required `LocationMessage`
fields and context; no protocol patch is needed. The original envelope remains
in the archive. The decoder repairs eligible saved unsupported messages once,
without changing message identity, read state or deletion/expiry guards.
Missing/out-of-range coordinates produce an unavailable-location message and
never become a map request. Zero coordinates are valid.

The public WhatsApp Web modules `WAWebParseLocationMessageProto`,
`WAWebMessageLocation.react`, `WAWebMapUtils` and `WAWebFormatLocationMsgText`
were inspected on 4 November 2026. They distinguish fixed and live locations,
display a map with optional place text, and construct Google Maps destinations.
Their static map service uses an application API key, separate from WhatsApp's
message transport. Reference URLs and hashes are in `prep/upstreams.json`.
No proprietary renderer code or API key is included in Heya.

The owner selected OpenStreetMap previews. Heya independently composes a small
static map from standard OSM tiles, with visible attribution. Only cards in the
bounded visible/nearby demand are requested; history import does not fetch maps.
The fixed-size image uses the existing preview worker and archive cache, so
resizing or receipt updates do not fetch tiles, decode images or move the card.
Maps are not replaced with enlarged low-resolution JPEG thumbnails. If loading
fails, the pin and map-opening action remain available; cached maps work offline.

Requests identify Heya, follow the HTTPS tile endpoint, and cache tiles for 30
days under schema 21 in the account archive, following its encryption setting.
The provider receives zoom/x/y tile indices, which reveal the approximate area;
message text, contact identities and place names are not sent. Opening a map or
place website is an explicit action. See the
[OpenStreetMap tile policy](https://operations.osmfoundation.org/policies/tiles/).
Only one finite download task runs at a time, with a three-second HTTP deadline
and a twelve-second budget checked between tiles. Cancellation stops further
requests; the current bounded HTTP request may finish before the task exits.
The existing preview cache handles failure cooldowns and rejects stale results
after deletion, expiry, lock or changed content. No new polling thread is added.

Synthetic tests cover coordinates, wrappers, archived-message repair, unchanged
read state, failed/stale preview handling, map edges, UI containment and stable
geometry. `HEYA_DEMO_LOCATIONS=1 scripts/ui-offscreen.sh --framebuffer
/tmp/heya-locations.png --demo-chat 7` uses a procedural map without network
requests. Native account interoperability and provider availability remain
separate from these tests.

Validation on Linux, 4 November 2026: 383 default-feature tests and 364 tests
without default features passed; strict application Clippy and the release
build passed. Private X11 checks exercised the composer menu, multiline Unicode
editing, image paste and file drop. Synthetic location screenshots and layout
tests covered light/dark themes and narrow windows.

Two optimized offscreen OpenGL comparisons showed no consistent change in the
10,000-message workload. Render CPU p95 ranges before/after were 11.59–13.39 /
12.96–13.04 ms for scrolling, 15.18–16.52 / 13.72–17.30 ms for resizing and
12.96–17.37 / 15.16–16.52 ms for composer edits. Existing cold rendering spikes
remain in both builds. These are application smoke measurements on a private
display, not desktop-compositor or live-account latency guarantees. macOS
runtime behavior was not tested here.

## Live-location cards

Received live shares use a separate static card: a 270 × 150 logical-pixel map,
centred sender avatar, “Live location not available” label, optional selectable
caption and the normal message footer. Clicking the map or label opens
“Live location is not available on this device. View location on your phone.”
with an OK button. It does not launch a fixed-coordinate map as though the
sender's position were current. The availability message applies to active,
stopped and expired shares alike, following Web's desktop limitation.

The official `WAWebMessageLiveLocation.react` module was inspected in the
retained official Web bundles on 5 November 2026. Its static map, 40-pixel avatar,
caption and phone-instruction popup informed this independently written UI.
Heya uses the existing OSM preview provider and bounded avatar cache. It displays
the position supplied in the message, without subscribing to live updates or
requesting the phone's final-location state. This is presentation support, not
live tracking or location sending. No SDK patch, periodic task or idle thread
is added. The map and avatar reserve their geometry before image arrival.

The normal adapter decodes `LiveLocationMessage` and legacy `LocationMessage`
with `isLive`, retaining captions, forwarding/quote context and original envelopes.
Saved phone-only live-location placeholders upgrade locally through content
repair revision 14. Identity, unread state, and deletion/expiry guards are
preserved. Missing or invalid coordinates keep the recognizable unavailable
card without requesting a map. The sidebar and quoted-message labels identify
live shares explicitly.

Synthetic fixtures and UI checks cover captions, empty captions, invalid
coordinates, group sender avatars, map/avatar arrival without reflow, popup
activation/dismissal, same-chat refresh, removal and stale clicks, plus light/dark
and narrow layouts. `HEYA_DEMO_LIVE_LOCATIONS=1 scripts/ui-offscreen.sh
--framebuffer /tmp/heya-live-locations.png --demo-chat 7` reproduces the card
without account data or network requests. Native account and macOS validation
remain separate.

Validation on Linux, 5 November 2026: all 407 non-ignored application tests passed
across the host-service, location-regression and final toolkit-UI runs (14 native
or account-specific tests remain ignored). Strict release Clippy, the optimized
build and the build without default media features passed. Private X11 captures
verified light/dark and narrow cards plus a real pointer click opening the
phone-instruction popup. The screenshots exposed a vertically centred map
colliding with its label; explicit positioning and an overlap assertion fixed it.

Both card variants now live behind the existing location-card condition, rather
than adding an independent conditional to every ordinary message bubble. This
keeps the reviewed pixels identical while avoiding unnecessary timeline work.
The final optimized OpenGL smoke run measured render CPU p95 of 12.74 ms for
scrolling 10,000 messages, 14.05 ms for resizing and 15.69 ms for composer edits.
The two pre-feature baseline runs measured 12.16–13.22, 17.14–18.05 and
15.39–19.16 ms respectively. These observations show no regression in this
synthetic workload; they are not compositor or live-account latency guarantees.
