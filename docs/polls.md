# Polls

Use **+ → Poll**, enter a question and at least two different answers, choose
whether multiple answers are allowed, and send. Heya retains the form if saving
fails and preserves the normal composer draft. Questions allow 255 characters,
answers 100 characters each, with up to 12 options.

Received and sent text polls show selectable answers, confirmed counts and vote
bars. Selecting an answer changes your vote; selecting it again removes it.
**View votes** lists the voters for each answer, with contact photos and names.
Results update while the dialog is open. Pending votes show “Sending vote…”;
counts change after confirmation. Failed or uncertain votes retain their
selection and explain the failure. Sending uses the existing durable outbox.

The adapter uses wacore's public poll crypto for live updates. It tries known
creator/voter PN and LID aliases independently. Authenticated history can supply
plaintext vote add-ons and a creation secret outside the message; both legacy
`poll_updates` and current `message_add_ons` are handled. Add-ons remain hidden,
never contribute separate messages or unread counts, and are reduced to the
latest valid ballot per voter, including empty ballots. Malformed updates do
not replace a valid vote. Deleted/expired poll content is not restored by votes.
Older retained envelopes are repaired once in batches of 100. If the original
history did not include a required secret or voter identity, the card explains
that phone history is needed.

A small SDK patch exposes the existing creation-send path with caller-owned
`SendOptions`, retaining Heya's durable message ID and returning the sender
namespace actually used on the wire. See `patches/poll-creation-options.patch`.
No poll-specific thread, timer, network client or crypto implementation is added.
Slint receives derived counts, selections and bounded voter pages; database and
encryption work stay on the existing archive worker. Unchanged timeline snapshots
are reused. Polls cannot be forwarded or edited as text.

Ordinary single/multiple-answer text polls are supported. Quiz, photo, anonymous
or add-an-option variants require the phone. When a readable text definition is
available, Heya shows it read-only with a phone notice; non-text-only option
formats remain unsupported. Advanced variants are not converted into ordinary
polls. Cached results work offline; sending and
voting require a connection.

The visual/format reference is WhatsApp Web's poll creation parser, poll update
history parser, option rows and message view, reviewed on 2026-11-05. The UI and
archive implementation are Heya's own. Protocol fields and cryptography come
from the pinned whatsapp-rust revision in `prep/upstreams.json`.

Reproduce the synthetic checks without opening an account:

```sh
cargo test --locked --bin heya polls::tests
cargo test --locked --bin heya polls::tests::poll_archive_measurement -- --ignored --nocapture
HEYA_DEMO_POLLS=1 scripts/ui-offscreen.sh --framebuffer /tmp/heya-polls.png --demo-chat 7 --size 1200x900
HEYA_BENCHMARK_POLLS=1 HEYA_TEST_RENDERER=winit-skia-opengl scripts/ui-offscreen.sh --ui-benchmark
```

The archive stress case stores 3,000 vote changes from 100 synthetic voters.
A partial unresolved-event index avoids revisiting settled ciphertext during
insertion; the results query reduces history to each voter's newest relevant
ballot before looking up aliases. These checks use no account or microphone.
The ordinary test profile's timing output is diagnostic, not an optimized UI
benchmark. Native timing comparisons use the optimized application and a private
Xvfb display. They do not measure real compositor presentation, hardware GPU
completion, account delivery or macOS interoperability.

Linux validation on 5 November 2026: the full application suite passed with
404 tests (14 deliberately ignored) and 385 without media (13 ignored), plus
the protocol storage integration test in both configurations. A further focused
regression covers recovery of a creator identity interrupted by shutdown, while
preserving an identity already captured by the SDK. Six SDK creation tests
passed, including actual synthetic DM/group wire encryption and caller-owned
message IDs. The SDK patch reproduces exactly at the pinned base.

Native input on private Xvfb verified typing into question/answer fields,
creating a demo poll, selecting multiple answers, removing a vote and opening
results. Toolkit tests cover rejected/delayed saves, repeated submission,
closing during a results query, late responses and narrow/light/dark geometry.
No account poll was sent automatically; live interoperability remains a manual
check.

Paired optimized Skia/OpenGL offscreen runs used the installed contact-sending
build as the baseline. Ordinary 10,000-message scroll render CPU p95 was
13.24 ms before and 13.15 ms with polls; warm resize p95 was 12.79/13.26 ms.
The separate 10,000-poll-card stress run measured 12.10 ms scrolling and
11.65 ms warm resizing. These are one local paired run, not a statistical
claim of improvement. Cold layout/resize outliers remain outside those steady
scroll/resize p95 values. No change to the owner's compositor was tested.

Poll creation uses its saved conversation as the dialog context. Routine live
chat snapshots, incoming messages and receipt updates preserve the question,
answers and keyboard focus; navigating to another conversation closes the form.
The live-publication regression types into the real inputs between snapshots
and reproduces the original unintended closure before the context check is fixed.

The refresh regression failed on the former dialog guard and passed after the
conversation-context fix (5 November 2026). Strict release Clippy, formatting,
and the no-media cargo check also pass. Offscreen paired scrolling render CPU p95
was 13.12/13.32 ms before/after. Resize measurements varied between runs
(13.77–18.17 ms render CPU p95); no compositor performance is established.
