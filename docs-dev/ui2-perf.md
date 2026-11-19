# UI v2 frame cost

How much CPU a frame of UI v2 costs on top of the classic UI, how to measure it, the budget and the parts of the runtime that keep an idle frame close to free.

## Measuring

Build optimised and serve it on a stand of your own:

```
cargo xtask build --only web --single --release
bash tools/playtest/m5/serve-one.sh <stand dir> <port> <bundle>
```

Then run the frame phases of the `ui2-perf` scenario against it:

```
UI2_PERF_ROWS=frames node tools/playtest/run.mjs --scenario ui2-perf --attach http://127.0.0.1:<port> --out <dir> --profile release
```

`UI2_PERF_ROWS=all` (the default) runs the scenario's other rows as well. The scenario (`tools/playtest/scenarios/ui2-perf-frames.mjs`) starts one game at 1920 x 1080, device scale 1, with the camera left where the game puts it, and holds every phase 12 s: the classic HUD idle and with the mouse moving, the classic technology tree, then UI v2's HUD and each of the techs, city, policies, diplomacy, economy and trade routes windows idle and with the mouse moving, the technology tree scrolled by the wheel, and the classic HUD again at the end as a drift check. It prints one table and writes `logs/ui2-perf/ui2-perf-frames.json`; it fails when a phase goes past `FRAME_LIMITS` (wide limits meant to catch a regression of several times, not noise).

Counters in a running client:

- `?stats=1`: `window.__civ5stats.counters` holds `frame.cpu_ms` (p50 of the last 10 s of whole-frame CPU time) and the `ui2.*` counters of the last frame (`ui2.cpu_ms`, `ui2.source_ms`, `ui2.keys_ms`, `ui2.input_ms`, `ui2.place_ms`, `ui2.prepare_ms`, ...).
- `?test=1`: `window.__civ5test.ui2Frames(reset)` returns the ring of the last 1024 frames, one row per frame with every phase in ms (app, cpu, source, runtime, bind, style, text, layout, keys, paint, resolve, js, input, place, prepare, probe) and the counts (bindings evaluated, restyled, layout runs, chunks painted, nodes visited, delta ops, input events, upload bytes). `reset` empties it, so a phase is `ui2Frames(true)`, wait, `ui2Frames()`.

On a machine of your own (a physical monitor, a real GPU): open the game with `?stats=1&test=1` added to the URL, then in the console `__civ5stats.counters["frame.cpu_ms"]`, `__civ5stats.frame` (`fps`, `cpu_ms`, `gpu_ms`), `__civ5stats.passes_full.Ui2` (`gpu_ms`, `draws`, `instances`), `__civ5stats.counters["ui2.fill_kpx"]` and, for one phase, `__civ5test.ui2Frames(true)`, wait ten seconds, `__civ5test.ui2Frames()`. The page's log says which backend it took (`renderer: selected backend`); `gpu_ms` exists on WebGPU only. Say whether the bundle is a dev or a release one: they differ by two to four times (see below).

`frame.cpu_ms` includes the game, the map and the browser's own work, so compare phases of one run, never runs of different days. The `ui2 total` column is the sum of UI v2's own phases and is the stable number.

## Baseline and budget

Headless Chromium on a desktop GPU, release single-thread build, p50 of `frame.cpu_ms` and the mean of UI v2's own phases (ms):

| phase | classic | v2 before | v2 after | ui2 before | ui2 after |
|---|---|---|---|---|---|
| HUD idle | 3.64 | 4.05 | 3.84 | 0.25 | 0.12 |
| HUD mouse | 3.64 | - | 3.83 | - | 0.15 |
| techs idle | 4.25 | 4.87 | 3.99 | 0.88 | 0.15 |
| techs mouse | 4.25 | - | 4.00 | - | 0.29 |
| techs scroll | - | 4.89 | 4.11 | 1.87 | 0.85 |
| city / policies / diplomacy / economy / trade routes idle | - | - | 3.8-4.05 | 0.11-0.24 | 0.11-0.12 |

Budget: the HUD idle under 0.5 ms over the classic HUD, a window idle under 1 ms, the technology tree scrolling under 3 ms, and no window losing more than 10 % of the frame rate. What is left of an idle frame is the model source (about 0.06 ms: reading the store's changes) and the skin script tick (about 0.02 ms).

## Why an idle frame is free

- **Dirty queues.** Bindings, styles, layout and paint run only on what the arena marked; an idle frame visits no node.
- **Key maps.** The hotkey map and the hotkey labels are rebuilt only when the arena's revision, the evaluated bindings or the restyled and laid-out nodes say something they read changed (`input/hotkeys.rs`, `KeysWatch`); a model delta, an island, a localizer or a state change touches the watch. Before, both were rebuilt every frame by walking the whole tree.
- **Host probes.** The host reads the runtime's texts, windows and skin state for the test hooks only on frames that did work (visited nodes or delta ops).
- **Culling.** Paint does not emit items whose clip leaves nothing (`ui2-draw` `Item::clipped_away`) and does not descend into subtrees whose clip is empty, so a scrolled list paints its visible rows only.
- **Hit testing.** A pointer move walks only the subtrees whose clip contains the point, and reuses per-chunk lists of hittable boxes (page bounds and clip) cached under the arena's revision, its marks and the anchors' placements; a box is checked exactly only when its bounds contain the point.
- **Input timing.** Input handled between frames is timed and counted into the next frame's stats (`ui2.input_ms`).

## GPU

The `Ui2` pass is timed on the GPU by timestamp queries where the device has them (WebGPU): `__civ5stats.passes_full.Ui2.gpu_ms` with `?stats=1`, `gpuMs` in `ui2Frames()`. Playwright's own Chromium cannot start WebGPU on Windows here (its Dawn misses `dxil.dll`) and falls back to WebGL2, which has no timestamps: run the scenario with `CHROMIUM_PATH` at an installed Chrome to get them. What the pass draws is counted on every frame whatever the device: draw calls, instances and the fill, the pixels its quads cover with overdraw (`ui2.fill_kpx`, per layer `ui2.fill_kpx.<layer>`, of it blurred box shadows `ui2.fill_kpx.shadow`; `fillKpx`, `shadowKpx`, `textureKpx`, `draws`, `instances` in the ring). The scenario reports them per phase (`UI2_PERF_DSF=2` for a device scale of 2, `UI2_PERF_VIEWPORT`, `UI2_PERF_UNCAPPED=1`) and checks `GPU_LIMITS` (draw calls, the fill in screens, the GPU time).

Release build, installed Chrome on WebGPU, desktop GPU (RTX 3070 Ti), 1920 x 1080 CSS px; GPU time of the `Ui2` pass in ms (sampled twice a second: about 0.05 ms of noise) and the fill in Mpx, before (the previous commit) and after:

| phase | DPR 1 before | DPR 1 after | fill before | fill after | DPR 2 before | DPR 2 after | fill after (DPR 2) |
|---|---|---|---|---|---|---|---|
| HUD | 0.15 | 0.10 | 1.52 | 1.11 | 0.52 | 0.24 | 4.3 |
| techs | 0.33 | 0.28 | 3.46 | 3.38 | 0.95 | 0.78 | 13.3 |
| city | 0.36 | 0.24 | 3.75 | 3.31 | 1.19 | 0.78 | 13.1 |
| policies | 0.57 | 0.39 | 6.13 | 5.95 | 1.61 | 1.42 | 23.6 |
| diplomacy | 0.27 | 0.14 | 2.69 | 1.82 | 0.80 | 0.44 | 7.2 |
| economy | 0.61 | 0.33 | 8.23 | 4.59 | 2.26 | 1.01 | 18.2 |
| trade routes | 0.66 | 0.34 | 7.26 | 4.18 | 2.03 | 0.89 | 16.6 |

Draw calls: 3 for the HUD, 1-2 per window, up to 18 for the technology tree (its pictures of game assets: one draw per 8 external textures); 300 to 3200 instances. A desktop GPU has room to spare; an integrated one is 20 to 50 times slower at fill, so the fill is the number to keep down: at DPR 2 the policies window is 2.8 screens of fragments.

What the GPU work is and what changed:

- **One fragment shader for everything**, so its size costs every fragment. The text shadow blur sampled the glyph atlas 81 times per pixel (and, in the previous commit, 289 times); it is now done once on the CPU when the glyph is rasterized (`ui2_text::GlyphKey::blurred`, `blur_bitmap`; SIMP-288 says how it differs from the browser's) and a shadow glyph is one texture read like any other. The shader lost its loops, and every phase got cheaper, not only those with text shadows.
- **Rings.** A border-only box (each `inset 0 0 0 1px` engraved line of the skin is one) covered its whole rectangle with fragments that draw nothing inside; a window's outer shadow (`0 24px 64px`) covered the window and its blur. Both are now drawn as four bands around the hole (`vs` in `ui2.wgsl`), so a window's line and shadow cost their edges only: economy and trade routes lost 3 Mpx each. Under a transform the quad stays whole.
- **Shadow pixels inside the element** return before the blur.
- **What is left** is mostly honest fill: the translucent backdrop over the map (one screen per window), the window's panel and the cards on it.

The threaded release bundle (`cargo xtask build --only web --release` with `GAMECORE_LIB_DIR_wasm32_threads`) measures the same on the main thread: 1.9-2.3 ms of frame CPU in installed Chrome on WebGPU, UI v2's own phases 0.08-0.11 ms idle and 0.6 ms scrolling the technology tree.

## Dev builds

A dev bundle (`cargo xtask build --only web --single`, no wasm-opt) was not a measure of the release. With every crate unoptimised, the classic HUD's frame took 13.9 ms of CPU against 3.6 in a release, UI v2's own phases were 3 to 6 times their release cost (0.4-0.5 ms idle, 1.5 ms with the mouse over the technology tree, 5 ms scrolling it) and the frame rate fell to 47-53 with the mouse over the technology tree or the policies and while scrolling.

The dev profile now builds dependencies at opt-level 2 and the crates of the frame (render, client-app, client-core, replica, ui2-runtime, ui2-draw, ui2-text, ui2-host, ui2-source, ui2-model) at opt-level 1 (`Cargo.toml`). Headless Chromium, WebGL2, 1920 x 1080, frame CPU p50 / UI v2's own phases (ms):

| dev bundle | classic HUD | v2 HUD idle | v2 techs idle | v2 techs mouse | v2 techs scroll | lowest fps | wasm |
|---|---|---|---|---|---|---|---|
| all at opt-level 0 | 13.9 | 13.6 / 0.50 | 13.7 / 0.56 | 20.7 / 1.47 | 13.9 / 5.0 | 46 | 81 MB |
| dependencies at 2 | 10-15 (noisy) | 15.6 / 0.70 | 11.1 / 0.61 | 12.6 / 1.28 | 12.6 / 5.3 | 54 | 65 MB |
| and the frame's crates at 1 | 5.9 | 6.3 / 0.24 | 6.4 / 0.29 | 6.5 / 0.56 | 6.4 / 1.66 | 60 | 49 MB |
| release (wasm-opt) | 3.6 | 3.8 / 0.12 | 4.0 / 0.15 | 4.0 / 0.29 | 4.1 / 0.85 | 60 | 28 MB |

What it costs to build (the wasm client alone, `cargo build --target wasm32-unknown-unknown -p app-web`, `CARGO_INCREMENTAL=0`): once, the dependencies again (the first `cargo xtask build --only web --single` after the change took 6.2 minutes); after touching ui2-runtime 15 s instead of 9, client-app 10 s instead of 6, render 28 s instead of 14. Most of a `cargo xtask build --only web` after a change is not Rust: the skins' conversion and wasm-bindgen take about 2.5 minutes of its 3.

## Text

Glyphs are rasterised by the automatic hinter in its light vertical mode up to 48 px (horizontal edges on whole pixels, x left at the outline's position), stored as linear coverage, and turned into alpha in the shader by a contrast exponent chosen per run from the text colour's luminance and its size (`ui2-draw` `glyph_contrast`): light text on dark panels and small sizes are thickened the most, which stands in for stem darkening. A text shadow blurs that alpha, as the browser blurs drawn text: on the CPU, once per glyph, into a sprite of its own in the atlas (`GlyphKey::blurred`). `tools/playtest/lib/sharpness.mjs` measures a crop (acutance: mean edge contrast; midtone: share of edge pixels left half-covered); `ui2-remarks` with `UI2_SHOT` writes crops of the top strip, the technology tooltip and a city banner with 4 x zooms, and `tools/playtest/ui2-mockup-shots.mjs` takes the same strip of a mockup in Chrome as the reference.
