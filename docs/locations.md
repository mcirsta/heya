# Received fixed locations

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
