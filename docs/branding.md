# Heya logo

The canonical logo is [`ui/assets/heya.svg`](../ui/assets/heya.svg): a rounded
white lowercase **h** in a teal (`#008f83`) speech bubble, with a transparent
background. Both shapes are paths; the logo needs no fonts or external files.
Keep its square view box and transparent padding when using it as an app icon.
The brand colour stays the same across application themes.

This is Heya's own artwork, recreated as vector paths from the first of three
image-generated concepts selected by the owner on 15 October 2026. It is not
derived from the ZapFast or WhatsApp logos. The concept image is a design
reference; only the SVG is consumed by the application.

Slint embeds the SVG for the native window icon and connection screen. The Linux
application ID, desktop entry and icon name are all `org.heya.Heya`, allowing
Wayland taskbars to find the desktop icon and X11 launchers to match the window.

After a release build, install the current binary and launcher for your user:

```sh
cargo build --release --locked
python3 scripts/install-desktop.py
```

The installer uses the already installed `rsvg-convert` and
`desktop-file-validate`. It copies the binary to `~/.local/bin/heya`, the desktop
entry to `$XDG_DATA_HOME/applications`, and the original SVG to
`$XDG_DATA_HOME/icons/hicolor/scalable/apps/org.heya.Heya.svg`. The default data
directory is `~/.local/share`. PNG fallbacks at 16, 24, 32, 48, 64, 128, 256 and
512 pixels are rendered directly from the SVG and installed into the matching
hicolor directories. Rerun the installer after rebuilding to update the copy.
Launching from the menu uses normal saved-session startup.

The template desktop entry in `packaging/linux` uses `Exec=heya` for future
packaging. The user installer writes an absolute executable path so it does not
depend on the desktop session's PATH. This is development desktop integration,
not a complete distributable package or a completed M4 milestone.

## macOS bundle icon

Finder and the Dock use the bundle icon rather than Slint's window-icon property.
The macOS packager now converts `packaging/macos/Heya.iconset` into
`Contents/Resources/Heya.icns` with Apple's `iconutil`, and declares
`CFBundleIconFile = Heya.icns` in `Info.plist` before signing the app. The previous
packager omitted both, producing the generic application icon.

The ten standard/Retina PNGs cover 16 through 1024 physical pixels, each rendered
directly from the canonical SVG. They preserve the logo's transparency and
padding. The PNGs are checked in; ordinary Mac builds require no additional image
tools. After changing the SVG, regenerate them using the existing Linux tool:

```sh
python3 scripts/macos-icons.py --generate
```

`python3 scripts/macos-icons.py` checks source and PNG hashes, completeness and
dimensions. macOS builds run this check before packaging, and the mounted-DMG
check uses `iconutil` to decode the shipped icon and verify every representation.
Launch the `.app` bundle for Finder/Dock integration. The corrected resources
still need visual verification on macOS; Linux asset checks cannot establish the
Dock's cached appearance.
