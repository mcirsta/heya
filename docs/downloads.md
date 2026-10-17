# Explicit attachment downloads

The message menu's Download action saves the original photo, audio, document,
video or sticker through the Linux desktop Save As dialog. This is separate from
automatic thumbnails and audio playback. It does not open files or send messages.

One save runs at a time, with a 2 GiB limit and a ten-minute network timeout.
Cancel stops the operation; Retry uses the original message and asks for a
destination again. Successful saves show Saved for three seconds, then dismiss
automatically. Failed/cancelled transfers keep Retry available. A new transfer
cancels the previous success deadline, so it cannot hide newer progress/errors.
Changing chats cannot redirect a transfer. A disconnected
client can export previously cached original photos/audio. Documents and other
uncached originals require a connection. Expired CDN references may require the
sender to resend the attachment; media re-upload recovery remains pending.

The native picker uses the [FileChooser portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.FileChooser.html)
and [Request cancellation](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.Request.html).
The Linux desktop needs `xdg-desktop-portal` and a matching file chooser backend
(KDE uses `xdg-desktop-portal-kde`). No shell command or blocking dialog thread is
used. X11 dialogs receive the window ID; Wayland currently uses an unparented
dialog. Windows/macOS save pickers are not implemented yet.

## Data path and lifetime

- Presentation captures chat/message identity and delegates to a separate async
  service. SDK types stay in `protocol_downloads`; Slint types stay in presentation.
- The archive worker validates visibility, expiry and the captured encrypted
  envelope. View-once content is rejected. Cached originals are hash-checked off
  the event loop; an invalid cache can fall back to the network.
- The SDK streams authenticated plaintext to a private same-directory temporary
  file, checking its MAC and encrypted/plaintext hashes. Host retries truncate
  and rewind the staging file. Manual saves have their own single transport
  permit, so they do not consume the automatic preview/audio transport permit.
- The final length and source envelope are checked again before publication.
  Flush/sync and rename happen off the event loop. Existing regular files are
  replaced only after verification and a metadata check that they have not
  changed during the operation. Symlink and directory targets are rejected.
- Normal cancellation, failure or window close removes the partial output and
  preserves an existing destination. Closing also clears the protocol source
  before the SDK ownership fence. Abrupt process termination can leave a private
  `.heya-download-*` file; there is no persistent transfer recovery or cleanup
  registry. The metadata check is not a general filesystem race guarantee.
- Saved files are ordinary plaintext files in the location explicitly selected
  by the user. Destination paths and completion records are not stored in the
  encrypted archive. Originals downloaded solely for export are not newly cached.

Progress uses a coalescing watch channel, with writes reporting at most every
100 ms. A single conditional window-level card displays status/Cancel/Retry.
It adds no per-message controls, polling timer, or timeline-model updates. One
async deadline exists only during the three-second success confirmation. Idle
services wait on channels; they do not create a dedicated worker thread. The
picker connection exists only while choosing a file.

## Validation — 16 October 2026

The full suite passed 146 application/service/UI tests and one protocol-storage
integration test; the real-keyring check remains opt-in. Strict all-target
Clippy and the media-disabled configuration check passed. Tests include:

- Actual SDK streaming with fragmented synthetic encrypted HTTP bodies,
  first-host failure/truncate/retry, and plaintext hash rejection. Buffered HTTP
  fallback panics in this fixture. No account or external server is involved.
- A 32 MiB streamed file beyond the earlier automatic-media limit; cancellation,
  `/dev/full`, destination changes, private permissions, missing/invalid targets,
  retained blocking-writer cleanup, and the 2 GiB bound without a large allocation.
- Offline cached export, invalid-cache recovery, duplicate starts, immutable
  retry source, chooser cancellation, disconnect, expiry and service shutdown.
- A real private D-Bus connection with a synthetic portal implementation,
  including a response before the SaveFile reply, local Unicode paths, cancellation,
  and non-file URI rejection. This establishes the wire contract, not compatibility
  with the owner's desktop portal implementation.
- The installed KDE portal/backend on a private X server, private D-Bus and
  isolated XDG directories: Heya's real demo menu opens Save As, saves the exact
  synthetic original, and cancels with no output and a visible Retry state. Save
  and cancellation use separate isolated runs. No owner file/dialog is inspected.
- The real Slint menu callback, containment at three window sizes, and 1,000
  progress publications without timeline layout or geometry changes. Protocol
  shutdown tests retain both audio/download source references to check release.

Optimized native synthetic UI measurements and captures are recorded in
[the UI audit](ui-source-audit.md). Automated UI checks run through
`scripts/ui-offscreen.sh` on a private X server and never access account data.
The native close/reopen regression passed four demo/no-session cases in 23–46 ms
without an idle audio thread. The owner then confirmed that a real received
attachment saved and opened correctly in the updated normal session. This
establishes that tested account/save path, not every attachment type, expired-CDN
recovery, or all compositor/dialog-parenting configurations.


### Download folder preference

Settings → Files → Downloads folder chooses the initial folder for Save As; Use
default restores the system Downloads location with a `Heya` subfolder. If the
system location already ends in `Heya` (case-insensitive), it is used directly.
The existing Rust `directories::UserDirs` resolves this location on Linux, Windows
and macOS; there is no Qt dependency. Linux uses XDG user directories. The native
folder chooser currently uses the Linux portal; Windows/macOS chooser adapters
remain part of their platform work.

The preference persists atomically with the other settings. Existing settings
files receive the default without migration or replacement. The default directory
is created only when saving an attachment. Save As still asks for the final file
name/destination and handles overwrite confirmation. A chosen directory is checked
before it replaces the previous preference; cancellation leaves it unchanged.
An in-progress save keeps the folder snapshot from when its chooser opened.
The portal receives a NUL-terminated `current_folder` byte array for file saves and
uses `directory=true`, `multiple=false` for folder selection. Private-D-Bus tests
exercise these actual wire options without opening a desktop dialog.

References: [portable user directories](https://docs.rs/directories/latest/directories/struct.UserDirs.html),
[native file-chooser options](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.FileChooser.html).
