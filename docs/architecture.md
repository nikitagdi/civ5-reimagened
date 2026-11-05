# Architecture

A Rust workspace (edition 2024): one server, one client that builds for the browser (wasm32) and natively, a
set of shared crates, and offline tools. The game rules and AI are not reimplemented: they are the Lekmod
gamecore, linked through a narrow C ABI.

## Crates

| Path | Crates | Role |
|---|---|---|
| `crates/shared/` | `protocol`, `wire` | client-server messages (protocol version 12), WebSocket frames with zstd, IPC frames |
| | `gamecore-sys`, `gamecore` | the C ABI (`gamecore_abi.h`), the C++ stub, and the safe Rust API over it |
| | `game-host`, `turntrace` | I/O-free game host (messages in, effects out; the fixture test world); per-turn timelines |
| | `lua-sys`, `sqlite-sys`, `loc` | vendored Lua 5.1 and SQLite; the game's text-key and localization engine |
| | `asset-format`, `build-support` | blob and manifest formats; C/C++ flags for build scripts |
| `crates/server/` | `server` | `civ5-server`: gateway, accounts, lobby, worker supervisor, admin API |
| | `recording` | per-game recordings, read by `tools/civ5-replay` |
| `crates/client/` | `client-core` | sans-IO client logic: session, app states |
| | `replica` | the browser's copy of the game: snapshot and patch loading, journal, digest checks |
| | `ui`, `ui-client`, `ui-text`, `exe-api` | the original Lua/XML UI: control tree and layout, its files and text, and the globals the game's executable gives that Lua |
| | `render`, `draw-list` | the wgpu renderer (WebGPU, WebGL2) and the UI draw list |
| | `assets`, `audio`, `jobs` | asset manifest and blob streaming; sound; a worker-thread pool |
| | `client-app`, `interim-ui`, `platform*` | frame loop assembly, the egui fallback screens, platform services (web, native) |
| `apps/` | `web`, `desktop` | the browser entry point (`apps/web/www` page files) and the native client |
| `tools/` | `asset-pipeline`, `granny`, ... | asset converter (`civ5-assets`), model reader, test and dev tools, `xtask` |

`cargo xtask deps` enforces the dependency rules between layers (`tools/xtask/deps.toml`); for example, only
`crates/server/server/src/worker/` may create a gamecore.

## Authority model

```text
 browser (each player)                         server
 +---------------------------+   WebSocket    +-----------------------------+
 | original UI (Lua) + render |<------------->| gateway: accounts, lobby,   |
 | replica gamecore (wasm)    |  orders, UI   |   journal ring, snapshots   |
 |  - applies the journal     |  messages,    |        |  IPC               |
 |  - checks digests          |  journal      | worker: the authoritative   |
 |  - never runs the AI       |<-- HTTP ------|   gamecore (Linux), all AI  |
 +---------------------------+  snapshots     +-----------------------------+
```

- **One authority.** For each game the server runs one gamecore in a worker process, single-threaded. Only
  it applies orders and runs the AI. The original UI's turn handshake becomes `EndTurn` / `CancelEndTurn`
  orders; other UI network messages travel as frames to the server.
- **Replica.** The replica serves only the interface. The browser loads the same gamecore compiled to wasm,
  because the original UI reads game state through synchronous getters. The replica loads a snapshot (`GCS1`), then applies journal batches (`GCJ1`)
  of the human phase that the server writes. It refuses to advance time or take input on its own.
- **Checks.** Some batches end with a `CHECK` record: a digest of the whole game state. If the replica's
  digest differs, or a batch is rejected, unreadable or missing, the client sends `GcResync` (`Diverged`,
  `Rejected`, `BadBlob`, `Gap`) and loads a fresh snapshot. The game goes on; the player sees a short reload.
- **AI phase.** The AI phase is not journaled. At the start of each human phase the server saves a snapshot,
  compresses it with zstd and announces it (a reference plus a patch against the previous one); clients fetch
  it from `GET /replica/<hash>`. On a late Huge save a snapshot is about 535 KB and a patch about 220 KB.
- **Reconnect.** A dropped WebSocket retries with backoff and resumes from the last batch it saw if the
  server's ring still has it (the last 1024 batches or 2 MiB); otherwise it loads a fresh snapshot.
- **Fog of war** is applied when drawing: every replica holds the whole game (see
  [limitations.md](limitations.md)).
- **Saves.** The worker autosaves; a crashed worker restarts from the last save. Saves use our own format,
  not `.Civ5Save`.

Compare the original game: lockstep, where every machine runs the whole gamecore and the AI, only inputs cross
the network, and any non-determinism is a desync the host has to repair.

## The gamecore boundary

The gamecore lives in the fork
[nikitagdi/Lekmod](https://github.com/nikitagdi/Lekmod/tree/reimagened/v34.15) (branch
`reimagened/v34.15`): Lekmod's `CvGameCoreDLL_Expansion2` sources plus `headless/`, which reimplements the
closed engine libraries the DLL expects and builds `libgamecore.a` with clang for Linux and wasm32 (plain and
threaded). Changes to Lekmod's files are optimizations and determinism fixes behind `LEKMOD_HEADLESS_*`
defines.

- `gamecore_abi.h` is a C ABI: create a game from a setup or a snapshot, pump time, apply commands, query
  state, save. `gamecore-sys` carries a byte-identical copy (`cargo xtask check` compares them) and a C++
  stub for builds without the real archive.
- The core reads game files through host callbacks (`HostServices`): on the server from the game data
  folder, in the browser from the `game-data` blobs of the asset pack.
- The core is single-threaded and is called by one thread at a time.

## Asset pack

`civ5-assets build` reads your install and writes:

- blobs named by the sha256 of their content (textures, meshes, animations, UI files, sounds, data tables);
- a manifest (the `manifest` pointer names the current one; `manifests.log` keeps the history) mapping ids
  such as `data:cursors` or `tb:groups-b2` to blobs;
- load groups, so the client fetches a small boot group first and the rest as it needs it.

Many textures are stored GPU-compressed (BC1/BC7); the pipeline can add ASTC variants, but support for devices
without BC textures is not finished (see [limitations.md](limitations.md)). The server serves
blobs as immutable `/assets/<hash>` files; browsers cache them across sessions and releases.

## Renderer

`render` is built on wgpu: WebGPU in Chrome (the main target), WebGL2 as a fallback (default on Firefox, or
`?gfx=webgl2`), and native backends for the desktop app. Its WGSL shaders reproduce the original's terrain,
water, lighting, units, cities and effects; the look is compared with captures of the original game. With
cross-origin isolation the web client runs threaded (`pkg-mt`, web workers for CPU jobs); otherwise it runs
on one thread. Graphics presets: Low, Medium, High (Low by default on touch devices).

The game screen is the original Lua/XML interface from your install, run against the replica. Sign-in, lobby
and room are HTML pages.
