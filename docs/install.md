# Install

What you need to build the asset pack and run a server. Hosting for friends is in [hosting.md](hosting.md);
every build command is in [building.md](building.md).

## The game

Below, `<Civ5>` is the game folder (in Steam: right-click Civilization V, Properties, Installed Files,
Browse).

- Sid Meier's Civilization V on Steam, build 1.0.3.279, with Gods & Kings and Brave New World. Lekmod itself
  requires "all available DLCs except for map packs" (its installation guide), which includes the
  civilization DLC: several Lekmod leaders need them. We have only tested an install with every DLC (all
  civilization and scenario DLC and the map packs); whether anything works with some DLC missing is not
  verified.
- The Enhanced User Interface (EUI) by bc1, installed as `<Civ5>/Assets/DLC/UI_bc1` (the EUI installer puts
  it there). The game screen in the browser is the game's own Lua UI in its EUI form, so the `ui` set needs
  these files. Lekmod supports EUI up to 1.28 (1.29 and newer are not supported by Lekmod). We have not
  recorded which 1.2x version our test install has, and we have not tried an install without EUI.
- Lekmod v34.15 in `<Civ5>/Assets/DLC/LEKMOD_v34.15`, installed as follows (Windows).

### Install Lekmod v34.15

Lekmod has no v34.15 release; v34.15 exists only as the upstream commit
[`bf3f4e9d`](https://github.com/EnormousApplePie/Lekmod/tree/bf3f4e9d3f9cd0bf4fc989a28680751c10ca7200). Take
the `LEKMOD/` folder from one of these two sources:

- **Our gamecore fork (recommended).** Its `LEKMOD/` folder is upstream's plus synchronization fixes in three
  Lua files (`Lua/Civilizations/Lekmod_italy.lua`, `Lua/Lekmod_global_dummies.lua`, `Lua/Lekmod_policies.lua`;
  listed in the fork's `NOTICE-REIMAGENED.md`). The server and the browsers run the Lua of your install, so
  this is the copy the project was tested with. Clone the fork as shown in [Sources](#sources) and use
  `../Lekmod/LEKMOD`.
- **Upstream.** Download
  <https://github.com/EnormousApplePie/Lekmod/archive/bf3f4e9d3f9cd0bf4fc989a28680751c10ca7200.zip> and
  unpack it; the folder is `Lekmod-bf3f4e9d3f9cd0bf4fc989a28680751c10ca7200/LEKMOD`. `export-gamedata`
  still puts the fixed `Lekmod_policies.lua` into the server's game data (see below), but the Italy fix
  (FIX-0003) is missing: upstream applies Italy's golden-age bonus for a finished policy branch only on the
  machine of the active player, so server and browsers can disagree and a replica gets repaired from a
  snapshot. This combination is not tested.

Then:

1. Install EUI first (see above), so that step 3 picks the EUI variants.
2. Copy the `LEKMOD` folder to `<Civ5>/Assets/DLC/` and rename it to `LEKMOD_v34.15`. The asset pipeline also
   accepts the other spellings of the same version (`LEKMODv34_15`, as upstream's guide writes it, or a
   folder left named `LEKMOD`), but not another version; `build` stops with an error when it finds none.
3. Run `ui_check.bat` inside `<Civ5>/Assets/DLC/LEKMOD_v34.15` once (double-click it). It fills `Lua/UI` of
   that folder with the UI files that match the installed EUI (upstream requires this from Lekmod v33 on).
   Run it again whenever you change EUI. Upstream's `linux_ui_check.sh` is not part of v34.15.
4. Start the game, open multiplayer and check that the lobby lists LEKMOD. Then take the database snapshot
   (below).

The pipeline reads every official layer it finds (base game, expansions, the DLC packs, then Lekmod).
`cargo run -p asset-pipeline --release -- scan` prints which Lekmod folder it reads.

## Toolchains

| Tool | Version | Needed for |
|---|---|---|
| Rust (rustup) | 1.95.0, picked up from `rust-toolchain.toml`, with `wasm32-unknown-unknown` | everything |
| Rust `stable` with `rust-src` | any stable with `rust-src` (or name one in `CIV5_MT_TOOLCHAIN`) | threaded web build |
| `wasm-bindgen-cli` | 0.2.129 (must equal the crate in `Cargo.lock`) | web build |
| wasi-sdk | 33 (`WASI_SDK`, default `../toolchains/wasi-sdk`) | web build (C/C++ crates for wasm) |
| binaryen `wasm-opt` | 116 or newer (`CIV5_WASM_OPT` or PATH) | `--release` web builds |
| C++ compiler | MSVC on Windows; clang 22 or 18 with libc++ on Linux | native builds |
| WSL2, Ubuntu 24.04 | distro name in `CIV5_WSL_DISTRO` (default `Ubuntu-24.04`) | Windows: gamecore and real server |
| Docker with Compose | | hosting |
| Node.js 22+, Playwright | `npm ci` in `tools/playtest` | playtests and benchmarks only |

```sh
rustup toolchain install stable --component rust-src --target wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129 --locked
```

### Windows 11 (tested)

Install Visual Studio Build Tools (C++), rustup, the Windows release of wasi-sdk 33 (for the web build), and
WSL2 with Ubuntu 24.04. Inside WSL install what the gamecore build and the Linux server need:

```sh
sudo apt install clang-18 llvm-18 libc++-18-dev libc++abi-18-dev make python3
# wasi-sdk 33 for Linux, unpacked to ~/wasi-sdk-33 (or set WASI_SDK inside WSL)
# rustup inside WSL as well: `cargo xtask dev --wsl` builds and runs the server there
```

### Linux

The same packages as the WSL list above, plus rustup and the Linux wasi-sdk 33. Everything runs natively; drop
`--wsl` from the commands. The server and the gamecore are built and deployed on Linux (Docker), but a full
local `cargo xtask dev` session on a Linux desktop has not been recorded by us.

### macOS

Not tested for building. You can play in a browser on macOS against a server hosted elsewhere. The asset
pipeline has code to find a macOS Steam library, but the Mac version of the game may lay out its files
differently; point `--game` at a folder that holds `Assets/` and `Resource/`.

## Sources

Put both repositories in one folder. The build looks for the gamecore fork at `../Lekmod` (or `LEKMOD_ROOT`):

```sh
git clone https://github.com/nikitagdi/civ5-reimagened
git clone -b reimagened/v34.15 https://github.com/nikitagdi/Lekmod
```

Without the fork everything still builds, with a stub gamecore; the asset pipeline and `cargo xtask dev`
(test world) work too.

## The game database snapshot

Several sets read the game tables from the database the game itself builds at startup.

1. Start the game once with Lekmod enabled and wait for the main menu. Quit.
2. Copy `Civ5DebugDatabase.db` and `Localization-Merged.db` from
   `Documents/My Games/Sid Meier's Civilization 5/cache/` into a folder of your own, for example `../snapshot`.

`tools/asset-pipeline/gamedata.lock` pins the sha256 of the snapshot the project was tested with. A different
snapshot (another game or Lekmod build, other DLC) goes on with a warning and records its own hashes in the
export. Set `CIV5_GAMEDATA_STRICT=1` or pass `--gamedata-lock <file>` to make a mismatch an error;
`cargo run -p asset-pipeline --release -- lock-gamedata --db ../snapshot --out <file>` writes a lock for yours.

## The asset pipeline

Run from the `civ5-reimagened` folder. The game folder is taken from `--game <dir>` (alias `--game-dir`),
else `CIV5_ROOT`, else the Steam install found on this machine (registry `SteamPath` on Windows,
`~/.steam/steam` or `~/.local/share/Steam` on Linux, every library in `steamapps/libraryfolders.vdf`).

```sh
# Client pack: renderer art, the original UI, the replica's game data, lobby art (one manifest)
cargo run -p asset-pipeline --release -- build --out ../assets-out/b2 --set b2 --set ui --set game-data --set web-lobby --db ../snapshot
# Game data for the server's gamecore (DB, Lua, maps), a folder for --gamedata-dir
cargo run -p asset-pipeline --release -- export-gamedata --db ../snapshot --out ../assets-out/gamedata
# Checks and size report of the pack
cargo run -p asset-pipeline --release -- verify --out ../assets-out/b2
cargo run -p asset-pipeline --release -- report --out ../assets-out/b2
```

`export-gamedata` replaces one Lekmod Lua file (`Lekmod_policies.lua`) with the fixed copy from the fork; it
finds the fork through `--lekmod-repo`, `LEKMOD_REPO`, `LEKMOD_ROOT` or a `Lekmod` folder next to the
repository. `--no-lua-override` exports the installed file instead. `cargo xtask assets --set <name>` runs a
single set. The other subcommands are listed by `cargo run -p asset-pipeline --release -- --help`.

Output: content-addressed blobs (named by their sha256) and a `manifest` pointer in `--out`. Keep the folder
outside the repository. Sizes measured on our machine: the input install layers are about 4.0 GB (base) +
1.2 GB (Gods & Kings) + 1.3 GB (Brave New World) + DLC; a full output folder took 3.8 GB on disk (18,299
blobs); a browser downloads about 400 MB of it on first start (184.5 MB over the wire with brotli, measured
for an earlier pack). `report` fails when the pack is over its 600 MB budget. We have not measured how long a
full pipeline run takes.

## Run it

```sh
cargo xtask gamecore --target all     # Windows: runs in WSL; Linux: drop --target all for linux+wasm+wasm-mt
cargo xtask dev --wsl --gamecore-lib target/gamecore --gamedata-dir ../assets-out/gamedata --assets ../assets-out/b2
```

Open <http://127.0.0.1:8080/>. Dev mode stores state in `target/dev-state`, creates an account at a new nick's
first login and uses the registration code `dev`. URL options: `?nick=alice&autostart=1` starts a game at
once; `?gfx=webgl2` forces WebGL2; `?preset=low|medium|high`; `&hud=1` shows the performance panel (F3).
Without `--wsl` and `--gamecore-lib`, `cargo xtask dev` runs the fixture test world.

## Troubleshooting

| Symptom | Fix |
|---|---|
| `no Steam install of Civilization V found` | pass `--game <dir>` (the folder with `Assets` and `Resource`) or set `CIV5_ROOT` |
| `Lekmod v34.15 is not installed in ...` | copy the `LEKMOD` folder to `Assets/DLC/LEKMOD_v34.15`, see [Install Lekmod v34.15](#install-lekmod-v3415) |
| warning about `gamedata.lock` | your snapshot differs from the reference one; fine unless you want it strict (see above) |
| `export-gamedata` asks for the Lekmod repository | clone the fork next to the repository, or pass `--no-lua-override` |
| `wasm-bindgen CLI version ... does not match` | `cargo install wasm-bindgen-cli --version 0.2.129 --locked --force` |
| `wasi-sdk 33 not found` | set `WASI_SDK` to the wasi-sdk root (the folder with `bin/clang++`) |
| `the threaded wasm build failed` | `rustup component add rust-src --toolchain stable`, or `cargo xtask build --single` |
| `wsl` errors | check `wsl -l`; set `CIV5_WSL_DISTRO` to your distro name |
| `dev --wsl: the real core needs the game data` | add `--gamedata-dir` and `--gamecore-lib` as above |
| grey or flat terrain | you are on WebGL2 (Firefox, or no WebGPU); use desktop Chrome, see [limitations.md](limitations.md) |
| units without animation blending, browser cursor | the pack lacks `client-data` (part of `b2`); rebuild with `--set b2` |
