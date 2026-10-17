# Heya project guidance

## Current phase

Implementation includes an offline Slint prototype and the first live text and
encrypted archive slice. Read README.md and docs/roadmap.md before extending it. M0 and M2 remain
in progress; do not present
proposed components or acceptance targets as implemented or measured behavior.

## Technical direction

- Rust application, Slint UI, oxidezap/whatsapp-rust protocol implementation.
- Linux first, followed by Windows and macOS.
- ZapFast supplies the visual reference: layout, colors, spacing and appearance.
  It may also inform interaction and general design ideas. Write Heya's behavior,
  state, backend, storage and media logic independently;
  do not port ZapFast's application implementation or adopt feature parity.
- Prefer using whatsapp-rust unchanged through its public API behind Heya's
  protocol adapter. Targeted protocol-library changes are authorized when needed;
  document the concrete API gap and keep patches isolated and pinned. Do not
  contort Heya's architecture merely to avoid a necessary upstream change.
- Use the exact reference revisions in prep/upstreams.json. Keep upstream types
  inside the protocol adapter and Slint types inside the presentation layer.
- Keep network, database, image decoding, and media work off the Slint event loop.
- Use synthetic data for demos and automated tests. Live-account tests must be
  deliberate and must not send messages automatically.
- Keep protocol credentials, message content, QR payloads, and private media out
  of logs, fixtures, screenshots, and version control.
- Preserve upstream notices for copied or adapted material. Record provenance.
- Choose dependencies for technical fit; permissive licensing is not a user
  requirement. Keep routine attribution without making licensing a development gate.
- The user installs system packages and tools themselves. Provide pacman/paru
  commands and explanations; do not install packages or change toolchains.

## Quality and responsiveness

- Prefer completing one feature's behavior, failure cases, tests and responsiveness
  over adding more partially validated features. Rework an existing design when
  measurements or bugs show it is inadequate.
- Exercise realistic synthetic input: multiline and long text, Unicode, empty
  states, changing model contents, repeated resizing and narrow layouts. Shared
  display rules must apply to both the demo and live paths.
- Check positions and containment as well as dimensions. Preserve full user
  content when producing shortened display summaries.
- Run automated screenshots and UI benchmarks offscreen with
  `scripts/ui-offscreen.sh`; they must not interfere with the owner's desktop.
  Ask before any visible native performance/compositor check.
- Reopen the owner's normal Heya instance without `--offline`, so its saved
  session connects automatically. Reserve offline startup for explicitly
  requested offline use; automated checks use synthetic data separately.
- Keep UI work bounded during background bursts. Measure native performance in
  an optimized build with the synthetic benchmark; distinguish render/application
  timings from compositor animation and end-to-end account behavior.
- State exactly what validation establishes. Mocked services and toolkit layout
  tests do not establish real keyring, desktop, network or account compatibility.

## Build preferences

- When building with Ninja, let Ninja choose its default parallelism. Do not
  pass a fixed job count such as `-j2`, `-j 2`, or `--parallel N`, and do not set
  `CMAKE_BUILD_PARALLEL_LEVEL` to cap Ninja, unless the user explicitly requests
  a limit. Prefer `ninja -C <build-directory>` or
  `cmake --build <build-directory>` without parallelism overrides.
- Cargo is the planned application build entry point. Commit Cargo.lock when
  the application workspace is introduced. Do not use floating Git branches for
  runtime dependencies.
