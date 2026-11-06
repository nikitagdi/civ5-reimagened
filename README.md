<h1 align="center">civ5-reimagened</h1>

<p align="center">
  Civilization V: Brave New World + Lekmod multiplayer, reimagined for the browser
</p>

<p align="center">
  <a href="LICENSE"><img alt="License: AGPL-3.0-or-later" src="https://img.shields.io/badge/license-AGPL--3.0--or--later-blue.svg"></a>
</p>

> **You need to own the game.** To play you need your own copy of Sid Meier's Civilization V on Steam
> (build 1.0.3.279) with Gods & Kings, Brave New World and the civilization DLC that Lekmod requires (we
> tested with every DLC), plus Lekmod v34.15 and the Enhanced User Interface (EUI, up to 1.28) installed
> into it ([docs/install.md](docs/install.md#the-game)). This repository contains no art, audio, video,
> text or data files from the original game; the tools convert files from your own installation on your
> machine; converted files are never uploaded.

## What is this

A multiplayer server and browser client for Civilization V: Brave New World with the Lekmod v34.15 mod. The
server runs the real Lekmod gamecore (the C++ rules and AI of the mod, compiled with clang); the browser runs
the game's own Lua/XML interface and a new WebGPU renderer, both fed from assets you convert from your install.

## Why

- **One server is the authority.** The whole simulation runs on the server: one gamecore applies every
  player's orders and computes the world and every turn. Players' machines do not simulate the game, so they
  cannot drift apart from each other: no desyncs between players, no host migration, no waiting for the
  slowest machine to finish the turn. The browser keeps only a read-only copy of the state for the
  interface, checked against the server and reloaded from it if it ever differs
  ([docs/architecture.md](docs/architecture.md#authority-model)).
- **Play in the browser, on any device.** Windows, macOS, Linux, and Android phones and tablets in Chrome
  (touch controls: tap, drag to pan, hold for right click). Tested on desktop Chrome with WebGPU and on
  Chrome for Android; the WebGL2 fallback has known terrain issues; Firefox, Safari and iOS are not tested
  yet. See [docs/limitations.md](docs/limitations.md#platforms).
- **Faster turns and loading.** Compared with the original game on the same PC (i7-14700KF, RTX 3070 Ti)
  and the same save (a real Lekmod v34.15 game: Huge, turn 95, 12 civilizations and 24 city-states):

| What | Original (Lekmod v34.15) | civ5-reimagened (Chrome) |
|---|---|---|
| End of turn: click to the next turn, 3 runs | 17.1-17.4 s | 1.9-2.1 s |
| Loading the save to a playable map | about 40-55 s | 12-14 s |
| Order to result over a 200 ms link, p50 / p95 | not measured | 201 / 229 ms |

The original plays every AI unit's move on screen before the turn ends (this game had Quick Movement off); ours
computes the AI turn on the server in about 1 s and shows the result. The original's network latency needs two
machines with two copies of the game, which we did not have. Method, caveats and our other measurements:
[docs/performance.md](docs/performance.md).

## Limitations

- The render is recreated to resemble the original, not pixel-exact, and has gaps (coast and river shapes,
  some effects, the strategic view is missing, WebGL2 terrain). List: [docs/limitations.md](docs/limitations.md).
- Multiplayer only: no single-player campaign, no scenarios, no hot-seat. Up to 8 humans per game, plus AI.
- Only Brave New World with Lekmod v34.15, game version 1.0.3.279. Other mods are not supported.
- The server needs the gamecore built from our Lekmod fork,
  [nikitagdi/civ5-reimagened-coregame](https://github.com/nikitagdi/civ5-reimagened-coregame/tree/reimagened/v34.15), branch
  `reimagened/v34.15`. Without it only a stub core and a test world run.
- Fog of war is applied by the client only: every browser holds the full game state. Play with people you trust.

## Quick start

The tested path is Windows 11 with WSL2 (Ubuntu 24.04), where the gamecore and the real server run.
Prerequisites (Rust 1.95.0 from `rust-toolchain.toml`, the wasm target, `wasm-bindgen-cli` 0.2.129,
wasi-sdk 33, MSVC, WSL2 with clang 18): [docs/install.md](docs/install.md).

1. Get both repositories side by side (the build finds the fork at `../Lekmod`, or set `LEKMOD_ROOT`):

   ```sh
   git clone https://github.com/nikitagdi/civ5-reimagened
   git clone -b reimagened/v34.15 https://github.com/nikitagdi/civ5-reimagened-coregame Lekmod
   cd civ5-reimagened
   ```

2. Install EUI (up to 1.28) and Lekmod v34.15 into the game: copy `../Lekmod/LEKMOD` to
   `<Civ5>/Assets/DLC/LEKMOD_v34.15` and run `ui_check.bat` there once. Exact steps, and the upstream
   download as an alternative: [docs/install.md](docs/install.md#the-game).

3. Take a game database snapshot: start the game once to the main menu with Lekmod enabled, then copy
   `Civ5DebugDatabase.db` and `Localization-Merged.db` from
   `Documents/My Games/Sid Meier's Civilization 5/cache/` into a folder, here `../snapshot`.

4. Convert your install into an asset pack (the Steam install is found automatically; `--game <dir>`
   overrides it):

   ```sh
   cargo run -p asset-pipeline --release -- build --out ../assets-out/b2 --set b2 --set ui --set game-data --set web-lobby --db ../snapshot
   cargo run -p asset-pipeline --release -- export-gamedata --db ../snapshot --out ../assets-out/gamedata
   ```

5. Build the gamecore archives (inside WSL on Windows) and run a local server with the real core:

   ```sh
   cargo xtask gamecore --target all
   cargo xtask dev --wsl --gamecore-lib target/gamecore --gamedata-dir ../assets-out/gamedata --assets ../assets-out/b2
   ```

6. Open <http://127.0.0.1:8080/> in Chrome and create an account (Create account tab) with the invitation
   code `dev`; this code works in dev mode only.

`cargo xtask dev` without `--wsl` and without the gamecore runs a fixture test world, which needs neither
the fork nor WSL. Details and troubleshooting: [docs/install.md](docs/install.md).

## Hosting

To play with friends, run the server on a Linux host behind a TLS reverse proxy. `cargo xtask dist` builds a
Docker build context; `deploy/compose.yaml` runs it; `cargo xtask deploy --target <name>` does the whole
deploy over ssh from a target file made from `deploy/targets/example.toml.template`. Friends register on the
site with a nick, a password and your invite code (`CIV5_PASSWORD`). See [docs/hosting.md](docs/hosting.md).

## Build from source

```sh
cargo build --workspace --locked   # native crates, stub gamecore
cargo xtask build                  # server, desktop app and the web client
cargo xtask check                  # fmt, clippy (native and wasm32), dependency rules, game-file check
cargo xtask test                   # cargo test --workspace
```

All commands, the gamecore build and the environment variables: [docs/building.md](docs/building.md).

## Documentation

- [Install and asset pipeline](docs/install.md)
- [Building](docs/building.md)
- [Hosting](docs/hosting.md)
- [Architecture](docs/architecture.md)
- [Performance](docs/performance.md)
- [Limitations](docs/limitations.md)
- [Legal and licenses](docs/legal.md)

## Contributing

Issues and pull requests are welcome. Run `cargo xtask check` before a PR and never commit game files; see
[CONTRIBUTING.md](CONTRIBUTING.md). Security issues: [SECURITY.md](SECURITY.md).

## License

AGPL-3.0-or-later ([LICENSE](LICENSE)) with an additional permission under section 7 to link the gamecore
([LICENSE-EXCEPTION.md](LICENSE-EXCEPTION.md)). The standalone tools `tools/asset-pipeline` and
`tools/shader-re` and the crates `asset-format`, `sqlite-sys`, `build-support` and `mod-manifest` are MIT
([LICENSE-MIT](LICENSE-MIT)). `tools/granny` is `MIT AND Apache-2.0 AND BSL-1.0`: our code is MIT, and two
files port code of nwn2mdk ([third_party/NOTICE-granny.md](third_party/NOTICE-granny.md)). The gamecore fork has its own terms. Details: [NOTICE](NOTICE),
[docs/legal.md](docs/legal.md).

## Legal

Unofficial fan project, not affiliated with, endorsed, sponsored or approved by Firaxis Games, 2K, Aspyr or
Take-Two Interactive. Sid Meier's Civilization V, Civ, Civilization, 2K Games, Firaxis Games, Take-Two
Interactive Software and their logos are trademarks of Take-Two Interactive Software, Inc.
