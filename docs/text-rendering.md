# Linux text coverage

Heya keeps the same text palette, font sizes, weights, font selection and layout.
The Linux Skia surfaces use `text_contrast = 0` and `text_gamma = 1`: the
rasterizer's grayscale coverage goes directly to antialiasing. Skia's default
surface correction made dark text visibly lighter than the pinned ZapFast
reference. Antialiasing, hinting, fractional glyph positions and image/surface
color spaces remain enabled and unchanged. This is a text appearance policy,
not a general improvement to Skia's rendering accuracy.

The setting applies when creating Linux OpenGL, Vulkan and software surfaces,
and to CPU snapshots. Skia's child surfaces inherit the parent properties,
including cached item/layer surfaces. There is no extra drawing pass, glyph
processing, cache, timer, worker or desktop-settings watcher. Other platforms
still receive `None`, preserving their original Skia defaults.

## Isolation

The experiment holds the font, size, color and text constant and varies hinting
and surface coverage independently. Run `tools/bench/skia_text_coverage.rs`
against Heya's pinned `skia-safe` after an optimized build. The source uses only
synthetic text and the installed Inter regular face. Its two lines use 14.5 and
21.75 physical pixels. These are font-rasterization measurements, not a general
percentage change in screen brightness or a contrast-ratio measurement.

At 14.5 pixels, integrated darkness (`sum((255 - red) / 238)` on white with
`#111b21` ink) was:

| Hinting | Stock surface | Unmodified coverage | Contrast 0 only | Gamma 1 only |
| --- | ---: | ---: | ---: | ---: |
| None | 947.8 | 1169.3 | 814.0 | 1319.5 |
| Slight | 943.4 | 1164.1 | 810.8 | 1310.9 |
| Normal (unchanged default) | 933.4 | 1166.0 | 794.9 | 1316.6 |
| Full | 933.4 | 1166.0 | 794.9 | 1316.6 |

Changing hinting at stock coverage shifts darkness by less than 2%; the neutral
coverage pair changes it by about 25%. Changing just one correction parameter
is insufficient. The selected pair is the identity setting, not a fitted
brightness constant. The default properties were contrast 0.5 and gamma 0;
gamma 0 lets Skia choose its correction for the destination color space.

The pinned ZapFast uses regular 14.5-pixel body text and `#111b21` in the light
palette. Its Linux `fastframe-text` configuration uses
`FontColorTransferFunction::Off`, slight hinting and fractional positions.
Reference revisions are recorded in `prep/upstreams.json`. The comparison uses
a small, independent egui/Glow fixture at that exact egui revision, with the
same installed font files. It is not a capture of the owner's live chat.

## Reproduce the Heya fixture

```sh
cargo build --release --locked --example text-rendering \
  --config 'profile.release.build-override.debug-assertions=true'
HEYA_TEST_BINARY=target/release/examples/text-rendering \
  HEYA_TEST_RENDERER=winit-skia-opengl \
  scripts/ui-offscreen.sh --framebuffer /tmp/heya-type-light.png --light
HEYA_TEST_BINARY=target/release/examples/text-rendering \
  HEYA_TEST_RENDERER=winit-skia-opengl \
  scripts/ui-offscreen.sh --framebuffer /tmp/heya-type-dark.png --dark
```

The fixture uses installed Inter and Adwaita Sans; install both to compare the
same families. The application does not require Adwaita Sans or bundle fonts.
The existing CPU-render/resize test also runs on Linux now, without a window or
GPU. Automated native captures and benchmarks use `scripts/ui-offscreen.sh`.

The standalone isolation tool can be built without another Cargo dependency
graph (adjust the build paths to the current optimized artifacts):

```sh
rustc --edition=2024 tools/bench/skia_text_coverage.rs \
  -L dependency=target/release/deps \
  -L native=target/release/build/skia-bindings-<hash>/out/skia \
  --extern skia_safe=target/release/deps/libskia_safe-<hash>.rlib \
  -o /tmp/skia-text-coverage
/tmp/skia-text-coverage /tmp/skia-coverage
```

## Isolated toolkit patch

Slint 1.18.1 has no public setting for these surface properties. The minimal
local patch is `patches/slint-skia-text-coverage.patch`, applied to the exact
crates.io `i-slint-renderer-skia` 1.18.1 source in
`vendor/i-slint-renderer-skia`. No Skia source or compile features change.

- Base crate SHA256: `dc9c1cf02385f1d26fc1f423544e52390a8e5c2d89420a8042aa94b4fe109d1f`.
- Base Slint commit: `372cf0ee5577c3dfec309a45e7b778ba4e81b734`.
- Preserve all upstream headers and `LICENSES/`; the other source files and
  normalized Cargo manifest are unchanged. Cargo packaging metadata, the
  standalone lockfile and `Cargo.toml.orig` are omitted.
- The patch hash and scope are recorded in `prep/upstreams.json`.
- Local only; not submitted upstream. An upstream API for surface text
  properties could replace this patch, leaving the policy in Heya.

To reproduce the vendored tree, extract the checksum-verified crates.io archive
`https://static.crates.io/crates/i-slint-renderer-skia/i-slint-renderer-skia-1.18.1.crate`,
omit the packaging files listed above, then apply the patch inside that crate
with `patch -p1`. Runtime builds use the checked-in source and require no patch
script or modification to Cargo's cache.

## Validation results

The same-font, same-size, same-color fixture was captured through the real
Slint/Skia OpenGL framebuffer on private Xvfb/Mesa. Integrated darkness compared
with the pinned egui/Glow fixture:

| Family / size | Previous vs reference, light | Fixed vs reference, light | Fixed vs reference, dark |
| --- | ---: | ---: | ---: |
| Inter / 14.5 px | −18.0% | +1.28% | +1.27% |
| Inter / 21.75 px | −12.4% | +1.30% | +1.28% |
| Adwaita Sans / 14.5 px | −16.3% | +2.21% | +2.16% |
| Adwaita Sans / 21.75 px | −9.4% | +3.95% | +3.91% |

The remaining differences come from distinct rasterizers/font handling; this is
close appearance matching, not pixel equality. All light samples still reach
exactly `#111b21`. Software and OpenGL captures differ by 0.0043% mean absolute
channel error over the fixture. No extra hinting or weight adjustment was added.

The default backend tried Vulkan, but Xvfb lacks DRI3 presentation support; the
check verified its OpenGL fallback. Vulkan code builds, but native Vulkan
pixels and compositor performance are not established by this run. macOS text
rendering retains its defaults and is not visually tested here.

The optimized default-feature suite passed 249 application tests plus the
protocol-store integration test (250 total; three deliberate skips), including
the real CPU-render/resize test newly enabled on Linux. Strict all-target
Clippy and formatting passed. The five desktop-service tests blocked by the
restricted runner passed when rerun with their private local sockets and PDF
sandbox subprocesses available; no account or visible desktop was used.

The synthetic emoji chat retained every one of its 4,635 saturated-color pixels
in the conversation area. This checks the sample's color artwork remains
unchanged; it is not a claim about every font or emoji platform.

Optimized OpenGL benchmarks used `scripts/ui-offscreen.sh --ui-benchmark`,
alternating the unchanged installed binary and the patched release binary in
this order: before 1, after 1, after 2, before 2, before 3, after 3. Each run
exercised 500 chats, up to 10,000 loaded messages, resizing, typing and changing
snapshots. Render CPU p95 timings in milliseconds were:

| Phase | Before 1 | After 1 | Before 2 | After 2 | Before 3 | After 3 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Changing updates | 11.51 | 12.53 | 12.14 | 12.63 | 12.44 | 59.40 |
| Scroll, 10,000 messages | 13.46 | 13.48 | 13.19 | 13.43 | 84.73 | 97.33 |
| Resize, 10,000 messages | 16.91 | 15.17 | 12.83 | 17.76 | 112.60 | 205.83 |
| Composer, 10,000 messages | 18.45 | 16.64 | 15.15 | 102.42 | 72.35 | 27.12 |

The comparable scrolling runs show no meaningful regression: p95 was
13.19–13.46 ms before and 13.43–13.48 ms after. Changing-update timings were
0.49–1.02 ms higher in the first two pairs; these runs do not establish whether
that small difference is systematic. Later runs had substantial external CPU
contention, with both binaries slowing sharply; unrelated compiler processes
were observed after the runs. These noisy results are retained rather than
treated as a rendering regression or a speedup. Broader performance conclusions
need a quiet host. No other workloads were stopped or modified.

Every benchmark completed shutdown and left no Heya or private Xvfb process
running. These are application/render measurements on an offscreen X11 surface;
they do not measure native compositor animation, Vulkan presentation or live
account latency.
