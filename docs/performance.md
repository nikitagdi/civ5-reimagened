# Performance

Every number here was measured, and each one names its baseline. None of them is a same-save, same-machine
comparison against the original game: we have not run one yet (see the last section).

## Simulation: AI turn time

**Setup.** A late Huge game: Lekmap Pangaea, 12 major civilizations (2 human) and 24 city-states, Emperor,
Online speed; saves from a real server, turns 125-130. Run under WSL2 on an Intel i7-14700KF (the gamecore
is single-threaded); other jobs were using the machine. Time is wall time from END TURN to the next human
phase.

**Baseline.** "Base" is our clang build of the same Lekmod gamecore commit without the optimization set;
"OPT" is the same commit with it. Base is not the original Visual Studio 2008 DLL.

| Turn | Base | OPT |
|---|---|---|
| 126 (quiet machine) | 14.0-15.2 s | 3.9-4.6 s |
| 125-130 of another save of the same game | 18.6 / 49.0 / 41.8 / 40.4 / 48.5 / 35.1 s | 2.8 / 3.3 / 4.2 / 6.7 / 6.1 / 7.8 s |

Process CPU time was 3.6x lower over 3 turns and 7.1x lower over 6 turns.

**Determinism.** For every turn and run, the game-state digest and the hash of the saved state were equal
between base and OPT. The optimization set is about 47 separate changes, each kept only if it changes no
result; they are listed in the fork's `headless/improvements/` registry.

**PGO.** A profile-guided build of the Linux archive (`cargo xtask gamecore --pgo`) takes turn 126 from
3.92 / 4.25 s to 3.48 / 3.52 s and turn 127 from 5.76 / 5.57 s to 4.92 / 5.04 s (1.13-1.15x), with equal
digests. Profiling the AI phase: unit moves take 76 % of it, path finding 62 %.

## End of turn as a player sees it

Same game, turn 126, OPT core, real worker process: END TURN to the new snapshot announcement took 4.0-4.2 s
(3.5 s AI, 0.5-0.7 s turn change, save and compression); turn 127 took 5.8 s on a quiet machine and 9.9 s on a
busy one. Switching the replica to the new turn took 0.20-0.39 s natively. Joining a late Huge game natively:
creating the core 741-849 ms, unpacking and loading the snapshot 275-328 ms. Browser join time was not
recorded.

## Original game, for context only

From a real network game on the original game (Lekmod v34.15, 2 humans, Huge, Quick speed, 10 AI majors and
24 city-states, turns 55-61), read from the host's network log: a turn took 55-87 s; the AI phase took 25-32 s,
of which 21-29 s was the game waiting for unit-move animations rather than computing; the human phase took
18-56 s; the end of turn took 3.7-5.7 s. It is a different save, at earlier turns, on different machines and
speed: an observation, not a like-for-like comparison with the numbers above.

## Render

**Setup.** The heavy benchmark scene (`cargo xtask bench`): a Huge map with 1500 units, 80 cities and 7 players,
no fog, heavier than a real game. High preset, 1920x1080, Chrome 148 with WebGPU, headed, Windows 11, Intel
i7-14700KF and an NVIDIA RTX 3070 Ti, 180 Hz monitor. Median of 3 runs, threaded client (8 workers), frame
rate uncapped.

**Baseline.** Our own renderer before the render optimization work, not the original game.

| Mode | FPS p50 before | FPS p50 after | 1 % low after |
|---|---|---|---|
| idle, default view | 10.6 | 155.8 | 112.5 |
| idle, far zoom | 12.9 | 255.8 | 158.9 |
| panning, far | 12.8 | 257.4 | 98.1 |
| panning, middle | 31.6 | 375.9 | 198.2 |
| zooming | 21.5 | 385.4 | 188.9 |
| several end turns | 21.4 | 215.5 | 120.8 |

After: CPU frame time at most 6.5 ms, GPU at most 4.2 ms; capped at 180 Hz it holds 179.9-180 FPS; GPU memory
1.11-1.26 GB. The image difference against the old renderer on the golden scenes did not change. The
single-threaded client still reaches at least 151.5 FPS p50.

**Likeness to the original.** On our golden scenes (captures of the original game) the FLIP image difference
is 0.249-0.381 (lower is closer; 0 is identical). This is a similarity measure, not a claim of equality.

## Asset delivery

Chrome, headed, network emulated at 100 Mbit/s and 20 ms; baseline our previous asset delivery of
the same asset sets:

| | Before | After |
|---|---|---|
| Cold start to the map | 16.2 s, 139.8 MB transferred | 10.8 s, 60.0 MB |
| Warm start (browser cache) | 7.8 s, 13.8 MB | 7.3 s, 3.0 MB |
| Pack size on the server | 428.1 MB | 401.4 MB (184.5 MB with brotli) |

## How to measure yourself

- Render: `cargo xtask bench` (Node.js 22+ and Playwright in `tools/playtest`); `--compare <baseline.json>`
  compares with an earlier run. `&hud=1` or F3 in the client shows a live performance panel.
- AI turns: the fork's `headless/tools/civsim` perf tools and `tools/pgo/compare.sh`, which plays saves with
  the plain and the PGO core and compares digests.
- Network: `cargo xtask nettest`.

## Not measured yet

- The original game's DLL and our core on the same save and machine (AI turn time).
- The original game's frame rate and load time.
- Bandwidth, ping and per-order latency of the current replica protocol.
- Frame rate on integrated GPUs, phones and tablets; any browser other than desktop Chrome.
- How long a full asset pipeline run takes.
