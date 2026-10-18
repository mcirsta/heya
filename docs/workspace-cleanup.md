# Source checkout and disposable outputs

Build Heya from the committed Rust/Slint sources with `cargo build --release
--locked`. `Cargo.lock`, `vendor/`, `patches/` and `prep/upstreams.json` preserve
the dependency versions and local patches. Native build requirements and commands
are in [development](development.md) and [macOS](macos.md). Normal builds do not
need reference checkouts, old test results, downloaded desktop applications or
previous binaries. Installers belong in GitHub release assets, outside Git.

## Clean the workspace

Stop any build first. Preview cleanup, then apply it:

```sh
python3 scripts/clean-workspace.py
python3 scripts/clean-workspace.py --apply
```

This removes `target/`, `.prep/` and Python bytecode under the source tools/docs.
Old screenshots, copied test executables, downloaded DMGs and raw validation logs
under `.prep/` are disposable. Historical documentation may name these paths;
they are records of a past check, not promised retained artifacts or build inputs.
Keep useful measurements in the versioned text reports instead of saving binaries.

To also remove the optional reference checkouts and supplied Windows application:

```sh
python3 scripts/clean-workspace.py --references --desktop-reference --apply
```

The script refuses tracked files, output symlinks, modified reference checkouts,
and reference revisions that differ from `prep/upstreams.json`. It never removes
Git history, `vendor/`, installed applications, global Cargo caches, account data,
Keychain/keyring entries, settings or personal downloads. Unknown files in
`.references/` are preserved. This is deliberately narrower than `git clean -xfd`.

Dev/test incremental compilation is disabled: repeated builds of the generated
Slint bindings had accumulated a 46 GiB incremental cache. This trades some rebuild
speed for disk space; debug information and release optimization are unchanged.
If needed for a short development session, `CARGO_INCREMENTAL=1 cargo build
--locked` opts back in. Clean those outputs afterwards.

## Reproduce build inputs and checks

- Runtime SVGs, palette JSON, macOS icon PNGs and the small synthetic media/PDF
  fixtures are source/build inputs, not compiled applications. Keep these inputs
  and upstream notices. The complete vendored crates retain their upstream source
  and test fixtures so local patches can still be reviewed and tested independently.
- `python3 scripts/macos-icons.py --generate` derives the macOS iconset from
  `ui/assets/heya.svg` using librsvg. `python3 scripts/macos-icons.py` verifies
  the source and generated hashes. Wallpaper generation is recorded in
  [third-party notices](../THIRD_PARTY_NOTICES.md).
- `python3 tests/fixtures/generate_previews.py` regenerates the synthetic PDFs
  with ReportLab. The thumbnail command is in [the fixture guide](../tests/fixtures/README.md).
- After building, `scripts/ui-offscreen.sh --screenshot /tmp/heya-light.png
  --light` produces a synthetic screenshot; `--startup-check`, `--close-check`
  and `--ui-benchmark` run the corresponding offscreen checks. Do not retain
  copies of the executable beside every result.
- [Reference checkout instructions](development.md#reference-checkouts) recreate
  the clean, pinned ZapFast and whatsapp-rust trees when another source audit or
  vendor refresh is needed. Windows app reverse-engineering notes remain in
  [the desktop audit](desktop-media-audit.md); the proprietary reference package
  is optional, owner-supplied, and never a build dependency or distributed asset.
- The macOS packaging script and CI create the `.app`, `.icns`, DMG and checksum
  from the tracked inputs. Published installers and CI artifacts can be downloaded
  again from GitHub; they do not belong in the source checkout.
