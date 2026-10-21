# Album resize investigation — 21 October 2026

The faded album thumbnails exposed Slint's Skia opacity-layer allocation issue,
also reported in [upstream issue 13834](https://github.com/slint-ui/slint/issues/13834).
`visit_opacity` used an unbounded `save_layer_alpha`, so a small translucent
subtree could allocate an intermediate surface covering the window's clip.
The layer is needed to composite overlapping children correctly; removing group
opacity or fading individual colors would change the appearance.

`slint-skia-opacity-bounds.patch` supplies the existing child-bounds calculation
as Skia's allocation hint. It includes overflowing/transformed descendants and
uses the renderer's scale factor. It adds no clipping, cache, worker or timer.
Heya's viewer UI and image resolution are unchanged. Patch reproduction and
SHA-256 are recorded in `prep/upstreams.json`; `vendor-slint-renderer.py` applies
the complete patch queue to the pinned source.

## Measurements

Optimized old/patched binaries ran sequentially, in alternating order, without
compilation. Every run used the same disposable SQLCipher archive: 10,000
messages, 100 album members, one loaded timeline row, a 2048px photo and seven
96px thumbnails. The resize sequence includes abrupt size resets and repeated
size requests; it is a stress test, not a simulated compositor animation.

| Metric | Before | Patched |
| --- | --- | --- |
| OpenGL resize render CPU p95, two runs | 16.61–16.87 ms | 13.34–13.99 ms |
| OpenGL resize render CPU maximum | 170.79–174.45 ms | 131.56–133.00 ms |
| OpenGL resize timer lateness p95 | 13.56–15.22 ms | 3.26–4.49 ms |
| Software resize timer lateness p95, paired run | 6.82 ms | 2.34 ms |

The OpenGL Xvfb driver reports missing DRI3 acceleration. An initial patched run
was much slower across every control phase; it is retained as variability evidence,
not used as the comparison. These timings do not establish native GPU completion,
Wayland/macOS behavior or compositor smoothness. Slint's rendering notifier also
disables partial redraws, so render CPU samples represent full repaint cost.
The software backend supplies no render callbacks; its render zeros are unavailable
counters, not measurements.

The benchmark now repeats resizing with the full viewer settled, without the
strip, without the main photo and with the viewer closed. This separates steady
resize from the final navigation publication that can overlap the first resize.
The final OpenGL run recorded settled-photo render CPU p95 9.96 ms and maximum
11.37 ms, with no snapshot publications in that phase. The earlier combined
navigation/resize phase still reached 129.59 ms. This does not establish that
every driver stall has been eliminated.

A separate 10,000-photo scroll pair recorded render CPU p95 10.63 ms before and
11.04 ms after; chat resize recorded 9.41 and 10.19 ms. These small differences
do not establish zero overhead. A later software run slowed across all resize
controls, including the empty viewer. Its repeat returned to resize timer p95
2.28 ms (settled 1.97 ms); both runs are retained as variability evidence.
Measurements and pixel checks are in
[`prep/validation/resize-20261021/measurements.json`](../prep/validation/resize-20261021/measurements.json).

```sh
HEYA_BENCHMARK_ALBUM=1 HEYA_TEST_RENDERER=winit-skia-opengl scripts/ui-offscreen.sh --ui-benchmark
HEYA_BENCHMARK_ALBUM=1 scripts/ui-offscreen.sh --ui-benchmark
```

The 1200×900 light OpenGL framebuffer is pixel-identical before/after. The
520×700 dark software capture differs by at most 1/255 in a color channel.
An actual Skia CPU regression checks overlapping group opacity, overflowing
children, rotation, an offscreen parent with a visible clipped descendant,
repeated resizing and 1×/2× scaling. No account data or visible desktop tests
are used.

Both synthetic suites passed (271 with media, 253 without; seven opt-in tests
ignored in each). Strict all-target Clippy passed in both configurations.

The current-master adaptation is [Slint draft PR 14307](https://github.com/slint-ui/slint/pull/14307).
Master's newer backdrop-blur branch remains unbounded to preserve filter input
behavior. The draft awaits Slint's contributor agreement and review. Full
master-workspace and native macOS validation remain upstream work.
