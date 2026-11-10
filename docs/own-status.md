# Status controls and quiet feedback

The icon left of **Chats** replaces the account-avatar shortcut. Its popup offers
Online, Invisible, Offline and Edit status message. Profile and connection settings,
including automatic connection, reconnect/relink and unlink, remain in the main
**Settings → Account** page. That page also offers the About editor.

The header uses a solid 34-pixel status badge with a 20-pixel symbol: a teal check
for Online, a grey crossed-out eye for Invisible, a grey minus for Offline,
teal reconnect arrows while working, and an amber exclamation mark for attention.
The symbol stays distinct without relying on colour. Light/dark foregrounds,
hover shading and a contrasting keyboard-focus border preserve legibility.
The heavier status glyphs follow the existing Lucide icon vocabulary and ISC
notice; popup actions continue to use the shared menu icons.

The old general notice banner and separate connection strip are removed, together
with their layout offsets. Copying text/images or loaded messages is silent.
Forward/edit/delete/star/pin queue confirmations and chat-management success labels
are discarded. Media errors already presented on their controls are not repeated.
Existing action dialogs retain their inline failures; an otherwise unpresented
user-action failure opens a compact dismissible, scrollable dialog. Dismissing it
restores editor focus where applicable. Automatic history fetches do not produce
empty-result/failure popups; an explicitly requested fetch can report a failure.
Retained-history recovery information is shown in Account settings only when needed.
These feedback changes do not hide actual deleted/call/pin/system message cards.

## Availability

- Online connects if necessary, then announces this device available.
- Invisible stays connected for messages and calls, announcing this device
  unavailable. It is not an account-wide guarantee: using another linked client
  or the phone can still make the account appear online. Heya does not silently
  change account-wide last-seen/online privacy settings.
- Offline uses the existing disconnect/save/release lifecycle. It also cancels
  connection attempts. It is independent of the account-operation queue.

The last successfully applied visibility is saved as the local `appear_offline`
preference. Selecting a mode before connecting saves that intent first. New
connections use the SDK's public `PresencePolicy::Manual`; the backend explicitly
applies the saved mode after the session is ready, so the SDK cannot automatically
announce Online during an Invisible reconnect. The icon combines real lifecycle
state with completed availability updates; it never treats command acceptance as
successful presence. Connecting/reconnecting/key-store waiting and failures use
icons and popup descriptions, with full connection errors also in Account settings.
There are no invented Away/Busy/DND protocol states.

The application announces `Available`/`Unavailable` through the existing SDK. This
is a device-level availability operation, separate from WhatsApp's
[online/last-seen privacy settings](https://faq.whatsapp.com/419827870318306/).
The SDK does not provide a peer acknowledgement proving what every other client
currently displays, and Heya makes no such claim.

## Status message

This edits profile About through `Client::profile().set_status_text`, and reads
it on demand with the public contacts API for the own PN JID. It does not post a
Status story. The editor bounds text to 139 Unicode characters without truncating;
server rejection keeps the entered text. Newer temporary About variants, emoji
metadata and expiry are not configured by this editor.

A missing/unavailable current value is not treated as a confirmed empty About.
Saving an untouched unknown value is disabled; entered replacement text remains
editable. Loading cannot overwrite typing, a delayed save cannot close a newer
edit, and a rejected save preserves the text. Closing during a submitted operation
closes the editor; it cannot unsend a request already accepted by the server.
The in-flight button therefore says Close rather than Cancel.

Profile text is neither logged nor written to the local preferences file. Only
visibility is persisted there. Each operation has one 12-second outer deadline;
source replacement/disconnection cancels it and releases client ownership. The
worker uses the existing async runtime and waits on commands/source/preferences;
there is no extra OS thread, keepalive, polling timer or retry service.

## Synthetic validation

```sh
cargo test --locked --bin heya own_account::tests
cargo test --locked --bin heya -- --exact presentation::tests::real_ui_callbacks_drive_the_background_service
cargo build --release --locked
HEYA_TEST_RENDERER=winit-skia-opengl scripts/ui-offscreen.sh --framebuffer /tmp/heya-status.png --size 1000x720 --light --click 31x30
scripts/ui-offscreen.sh --close-check
```

The protocol-free fake covers delayed/failed presence, reconnect into Invisible,
deadlines, cancellation, About failure/retry and character bounds. Real UI
callbacks cover duplicate submissions, confirmed-mode publication, stale profile
loads/saves, text preservation, silent copying and stable chat geometry. Demo
mode uses an in-memory fake and never changes a real profile or presence. Screenshots
and benchmarks use the offscreen wrapper, not the owner's desktop. These checks
do not establish WhatsApp-account interoperability or native macOS/Windows behavior.

On 2026-11-06 the synthetic suite passed: 416 tests, no failures, 14 deliberately
ignored native/manual checks. The release build, strict Clippy check and
no-default-features build check passed. The status popup is disabled behind
modal controls and closes when a modal opens; the call keyboard-traversal test
also exercises this regression. Light and dark offscreen captures were inspected.

A sequential optimized-build comparison against the previously installed build,
using the private X11 display and the same Skia OpenGL/llvmpipe renderer, gave:

| p95, milliseconds | Previous | Updated |
| --- | ---: | ---: |
| Unchanged snapshot application | 0.21 | 0.22 |
| Resize render CPU | 22.71 | 21.88 |
| Scroll with 10,000 messages, render CPU | 13.29 | 13.41 |
| Resize with 10,000 messages, render CPU | 17.17 | 18.30 |
| Typing with 10,000 messages, render CPU | 16.43 | 17.87 |

Both builds measured zero timeline rows during unchanged snapshot updates.
This single comparison shows comparable costs, not a compositor/native-GPU or
macOS performance guarantee. Compilation and other UI checks were stopped during
these timing runs.
