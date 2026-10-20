# Message context controls

The pinned ZapFast revision in `prep/upstreams.json` is the visual/interaction
reference. Source inspection covered `src/ui/conversation.rs` (right-click row
bounds, `context_menu`, `quick_reactions`, `selection_bar`), `src/ui/widgets.rs`
(menu frame/items/separators), `src/ui/message_info.rs`, `src/ui/dialogs.rs` and
`src/app.rs` (15-minute edit/48-hour sender-revoke eligibility and action handling).
Heya's archive, state transitions, outbox and media handling are independent.
Existing ZapFast/Lucide notices remain in `third_party/licenses/`. The context
menu forward glyph adapts the existing forward SVG to a single arrowhead; the
hover forward control retains its previously requested double arrow.

The right-click card combines a 34-pixel reaction row and 28-pixel action rows
with 6-pixel vertical spacing, 13.5-pixel labels and 16-pixel icons. Reaction
choices use existing session usage preferences, unique defaults and the owner's
current reaction when absent; the plus opens the shared full picker. The
existing chevron retains its native WhatsApp-style menu appearance and gains the
same implemented controls. Conditional overlays exist only while open.

One native window input handler intercepts right press/release before nested
Slint media controls consume it. Hit testing uses the timeline's existing height
index and exact cached bubble prefix. Date chips, headers, composer, modal
layers and transfer-status overlays do not open a row underneath. Release must
still be over the original message. No per-message input polling, extra worker
or idle timer is introduced. Inactive selection decorations and callbacks stay
out of the scene; the rounded composer outline exists only while focused.
Selection uses a bounded ID set and does not alter
row widths or invalidate measured heights.

The separate edit field preserves the composer draft. Saving checks ownership,
message age, current text and eligible text envelope again on the archive worker.
Sender deletion checks ownership and age too. Both operations have distinct,
stable stanza IDs in the existing serial outbox. Only ACK/receipt evidence
commits the local mutation; transport completion alone is ambiguous. An
uncertain edit/revoke is not auto-retried. Pending/failure states are displayed.
Repeated edits use increasing mutation versions; revocation always wins.

Incoming direct-message keys may name the receiving account. The authenticated
outer envelope owns chat/author routing; group participant ownership is checked
after alias canonicalization. Both incoming changes and acknowledged local
controls use the same mutation writer/projection. Missing originals produce one
visible deleted-message tombstone, and later history only restores its position
and author metadata. The sidebar and bubble use **This message was deleted**.
Startup reprocesses retained delete envelopes rejected by the former key check.

Delete for me commits a local wire-key tombstone and clears body/media/quote
references atomically. Its existing-outbox operation calls the SDK's public
`chat_actions().delete_message_for_me`, which syncs the owner's linked devices.
That operation is idempotent and can retry after an uncertain result. Incoming
app-state deletes, early events, history replay and PN/LID merges honor the
same tombstone. Delete/receipt sync for unknown chats stores an unlisted stub,
so it cannot invent contact-list entries. Unclaimed outgoing sends/edits are
cancelled when hidden.
A send already in flight may still arrive remotely. Expiry clears queued edit
payloads, and applied edits do not leave duplicate edit text in the control
row. The archive remains SQLCipher encrypted.

Batch hide/forward validation is transactional: one stale, invalid or duplicate
source rolls back the batch. Up to 100 selected messages can be copied, forwarded
to one destination or deleted for me. Forwarding still requires an explicit
recipient and confirmation. Selection and editing preserve unsent drafts.

Message info observes actual recipient delivery/read/played evidence while its
single dialog is open. The archive batches evidence from each receipt packet
and resolves known PN/LID aliases. Missing timestamps stay unavailable; opening
the dialog does not fabricate receipt times or start a network polling task.
Historical receipt retrieval and complete group-participant coverage remain
pending. SDK validation may reject community-announcement edits without an
original message secret; regular direct/group text uses its public edit API.
No protocol-library modification was needed for this slice.

The composer focus outline now takes its corner radius from the rounded pill,
instead of the intermediate rectangular content container.

Validation evidence is recorded in `docs/ui-source-audit.md` and locally under
`.prep/validation/context-menu-20261017/`. Automated validation uses synthetic
accounts/messages and the private offscreen display. Live edit/delete server
acceptance is not established by these tests.
