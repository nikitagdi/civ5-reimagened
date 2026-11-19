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

## Text

Glyphs are rasterised by the automatic hinter in its light vertical mode up to 48 px (horizontal edges on whole pixels, x left at the outline's position), stored as linear coverage, and turned into alpha in the shader by a contrast exponent chosen per run from the text colour's luminance and its size (`ui2-draw` `glyph_contrast`): light text on dark panels and small sizes are thickened the most, which stands in for stem darkening. Shadows blur that alpha, the same as the browser blurs drawn text. `tools/playtest/lib/sharpness.mjs` measures a crop (acutance: mean edge contrast; midtone: share of edge pixels left half-covered); `ui2-remarks` with `UI2_SHOT` writes crops of the top strip, the technology tooltip and a city banner with 4 x zooms, and `tools/playtest/ui2-mockup-shots.mjs` takes the same strip of a mockup in Chrome as the reference.
