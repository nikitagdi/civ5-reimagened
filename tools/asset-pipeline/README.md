# asset-pipeline (`civ5-assets`)

Offline converter: the files of your own Civilization V install (and a DB snapshot taken from the running game)
to content-addressed blobs under one manifest, which the server serves to the client (`--assets-root`).
Nothing it produces belongs in git: keep the output folder (default `../assets-out`) outside the repository.

## Inputs

- **The game folder** (`Assets/`, `Resource/`, `Assets/DLC/...`): `--game <dir>` (alias `--game-dir`), else
  `CIV5_ROOT`, else the Steam install found on this machine (Windows: `SteamPath` in the registry; Linux:
  `~/.steam/steam`, `~/.local/share/Steam`; macOS: `~/Library/Application Support/Steam`; plus every library of
  `steamapps/libraryfolders.vdf`). The game needs the expansions (Gods & Kings, Brave New World), the DLC that
  Lekmod requires, EUI (`Assets/DLC/UI_bc1`) and the Lekmod v34.15 DLC folder (`Assets/DLC/LEKMOD_v34.15`; other
  spellings of v34.15 such as `LEKMODv34_15` are accepted, and the sets stop with an error without one), see
  [docs/install.md](../../docs/install.md#the-game).
- **A DB snapshot** (sets `b1`, `b1-*`, `b2-units`, `b2-landmarks`, `b2-city`, `b2-misc`, `game-data`,
  `web-lobby`, and `export-gamedata`): a folder with `Civ5DebugDatabase.db` and `Localization-Merged.db`, copied
  from `Documents/My Games/Sid Meier's Civilization 5/cache/` after starting the game once to the main menu with
  Lekmod enabled. Pass it with `--db <dir>` or `CIV5_DB_SNAPSHOT`.

`gamedata.lock` pins the sha256 of the reference snapshot the project was tested with. A snapshot with other
hashes (another game build, Lekmod build or DLC set) goes on with a warning and its own hashes. To make a
mismatch an error, set `CIV5_GAMEDATA_STRICT=1` or name a lock with `--gamedata-lock <file>`;
`civ5-assets lock-gamedata --db <dir> --out <file>` writes a lock for your snapshot.

## Commands

```sh
# Client art and data for the B2 renderer (terrain, water, light, units, cities, effects, cursors, blend graphs)
cargo run -p asset-pipeline --release -- build --out ../assets-out/b2 --set b2 --db <snapshot dir>
# The original UI (Lua/XML, textures, fonts, sounds) and the lobby art
cargo run -p asset-pipeline --release -- build --out ../assets-out/b2 --set ui --set web-lobby --db <snapshot dir>
# The game data of the server's real core (DB, Lua closure, maps) as a folder for --gamedata-dir
cargo run -p asset-pipeline --release -- export-gamedata --db <snapshot dir> --out ../assets-out/gamedata
# Checks and sizes of an output folder
cargo run -p asset-pipeline --release -- verify --out ../assets-out/b2
cargo run -p asset-pipeline --release -- report --out ../assets-out/b2
```

`cargo xtask assets --set <name> [--game <dir>] [--db <dir>] [--out <dir>]` runs one set the same way. Other
commands: `scan`, `probe`, `extract`, `gltf`, `gc`, `precompress`, `seal-gamedata`, `tex-audit`, `repack`,
`variants`, `fgx-check`, `ui-vfs`, `ui-override`, `ui-loc-diff`, `lock-gamedata` (`--help` of each).

The set `client-data` (part of `b2`) writes the mouse cursors (`data:cursors`, from `Assets/UI/Cursors/*.ani`)
and the unit animation blend graphs (`data:dge-graphs`, from the `.dge` files of
`Resource/Common/UnitModels.fpk`). Without them the client keeps the browser's cursor and the units play the
clips of their actions.

`export-gamedata` applies FIX-0004 (the `Lekmod_policies.lua` of the Lekmod repository replaces the installed
one): it needs `--lekmod-repo <dir>` (or `LEKMOD_REPO` / `LEKMOD_ROOT`), or `--no-lua-override` to export the
installed file.

## Tests

`cargo test -p asset-pipeline` runs on synthetic fixtures (small files the tests make themselves). The tests
with real data run only when their variables are set: `CIV5_ROOT` (the game folder), `CIV5_DB_SNAPSHOT`,
optionally `CIV5_CIVHOST_LOG`, `CIV5_PSB_DIR`.
