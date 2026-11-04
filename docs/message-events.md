# Message events and phone-only content

Legacy and secret-encrypted message edits use the existing durable mutation
writer. They do not create a visible bubble, unread message or sender receipt.
An edit received before its original survives restart; arrival of the original
applies the pending change. Retained encrypted edits use whatsapp-rust's public
authenticated decryption API and the original message secret, with PN/LID
fallback. A failed decryption preserves the original. Sender ownership,
revision ordering, deletion, expiry and media-caption preservation still use
the shared mutation rules. Text edits retain the original message secret for
subsequent encrypted edits. No additional worker or retry timer is introduced.

Poll-vote updates and encrypted modification envelopes stay hidden with their
raw envelopes retained where they cannot yet be materialized. Poll rendering
and vote aggregation remain a separate feature.

Received history-sharing notices use the existing centered system-notice UI.
As in the inspected WhatsApp Web parser, the notice is hidden for its sender
and for accounts receiving the history. Other group members see who shared
message history. Missing metadata does not invent a visible notification.

`MASK_LINKED_DEVICES` is an intentional primary-device restriction. Heya shows
“One-time code. Open WhatsApp on your phone to view it.” Other phone-only
business messages and live-location shares have explicit explanations. Heya
does not attempt to retrieve the restricted code or present a live share as a
fixed location. The current WhatsApp Web live-location renderer also directs
users to their phone.

Startup event decoder revision 1 replaces the previous deletion-only repair.
It maps retained control events through the live/history adapter in transactions
of at most 100 rows, hides obsolete control bubbles, removes their unread flags
and queued sender receipts, and recounts affected chats. It preserves identities,
positions, drafts and raw unavailable content, and excludes deleted, expired and
mutated rows. Pending edits use the same mutation storage as new arrivals.
The completion marker prevents rescanning on every launch.

Synthetic regressions cover late originals, encrypted archive reopen, duplicate
replay, PN/LID identities, authentication failures, revocation, legacy startup
repair, phone-only text and history-notice visibility. The shared offscreen UI
test exercises the new notice with existing theme and layout checks. No account
message is sent by these tests.

Linux validation on 4 November 2026: 388 default-feature tests and 369 tests
without default features passed, including the shared offscreen UI suite.
Strict release Clippy for all targets and the optimized application build passed.
These checks use synthetic data; macOS runtime and real-account edit delivery
were not exercised by the automated tests.

The optimized private-display benchmark completed with 10,000 messages. Render
CPU p95 was 13.31 ms for scrolling, 13.34 ms for resizing and 16.46 ms for composer
edits, within the variation of the preceding comparisons. Unchanged snapshots
rendered no frames and measured no rows. These timings exclude the desktop
compositor and do not establish live-account latency; existing cold rendering
spikes remain outside the steady scrolling measurements.
