# Legal

Not legal advice; this page says what the project does and under which terms its files are.

## No game assets

This repository contains no art, audio, video, text or data files from the original game: no textures,
models, animations, sounds, music, UI files, maps, saves or game database. To play you need your own legally
obtained copy of Sid Meier's Civilization V with Gods & Kings, Brave New World and the DLC that Lekmod
requires, plus Lekmod v34.15 and the Enhanced User Interface installed into it (see
[install.md](install.md#the-game)). Lekmod comes from its author's repository or from our gamecore fork of
it; EUI from its author.

A few small text files hold names and facts read from the game, which the code needs to talk to the game's
own files; they hold no game content:

- `crates/client/exe-api/data/events.tsv`: the names of the Lua events the game's executable registers.
- `tools/granny/data/fgx-names.txt` and `member-names.txt`: material, shader-parameter and Granny structure
  member names, collected from the game's model and material files (stripped files store them only as hashes).
- `tools/shader-re/status.toml`: the names and sha1 hashes of the game's shader entries and the status of our
  port of each.
- In the gamecore fork: `headless/tools/clang/runtime/lua/gameinfo/civhost_reference.txt` (row counts and hashes of the game
  database, used to check a snapshot).

`cargo xtask check` fails when the git index holds a file with a game-file extension (`.exe`, `.dll`, `.dds`,
`.gr2`, `.fpk`, `.civ5save`, `.db`, `.fxsxml`, `.wav`, `.ogg`) outside `third_party/`, and `.gitignore`
excludes the pipeline's outputs.

## What the asset pipeline does

`tools/asset-pipeline` (`civ5-assets`) runs on your machine and:

1. finds your install (or takes `--game`) and reads its packs and folders: textures, models, animations,
   materials, UI files, fonts, sounds, maps and Lua scripts;
2. reads the database snapshot that your own copy of the game wrote to your Documents folder;
3. converts them into formats the browser client can load and writes them to a folder you choose
   (default `../assets-out`), outside the repository.

The pipeline does not send anything over the network. When you host a server, `cargo xtask deploy` copies
your converted pack to your own host, which serves it to the browsers of the players in your games. Each
player should own the game.

## Our code

The repository holds code written for this project: the server, the client, the renderer, readers for the
game's file formats, and a reimplementation of the interface that the game's executable gives to its Lua UI.
The renderer is made to look like the original by comparing frames, not by shipping its files.

The WGSL shaders of the renderer (`crates/client/render/src/shaders/`) were written for this project by
studying the compiled shaders of the game (reverse engineering for interoperability); they contain no shader
bytecode or source from the game. Many of them follow the structure of the game's shader stages (entry
names, register names, the order of the computation) and say so in their headers. `tools/shader-re`, which
reads the compiled shaders of your own install for that study, ships no game shaders either.

## Licenses

| Part | License |
|---|---|
| The program as a whole | GNU AGPL v3.0 or later ([LICENSE](../LICENSE)) |
| Additional permission (section 7) | linking with the gamecore built from the Lekmod/Firaxis sources, and the Lua and SQLite libraries it uses ([LICENSE-EXCEPTION.md](../LICENSE-EXCEPTION.md)) |
| `tools/asset-pipeline`, `tools/shader-re`, `crates/shared/asset-format`, `crates/shared/sqlite-sys`, `crates/shared/build-support` | MIT ([LICENSE-MIT](../LICENSE-MIT)) |
| `tools/granny` | `MIT AND Apache-2.0 AND BSL-1.0`: our code is MIT; `src/curve.rs` and `src/oodle1.rs` port code of nwn2mdk under Apache-2.0 ([LICENSE-APACHE](../tools/granny/LICENSE-APACHE)), and `oodle1.rs` also under the Boost Software License 1.0 its source file states ([LICENSE-BOOST](../tools/granny/LICENSE-BOOST)); see [NOTICE-granny.md](../third_party/NOTICE-granny.md) |
| The gamecore fork ([nikitagdi/civ5-coregame-reimagened](https://github.com/nikitagdi/civ5-coregame-reimagened)) | its own terms: Lekmod's terms for the upstream files and changes to them, MIT for the project's own files under `headless/`; see its `NOTICE-REIMAGENED.md` |

The `license` field of each `Cargo.toml` is authoritative for its crate. AGPL means: if you run a modified
server for other people over a network, you must offer them the source of your modified version. The gamecore
is not covered by the AGPL; it is linked only when you build it yourself.

## Third-party components

Listed in [NOTICE](../NOTICE): Lua 5.1.5 (MIT), the SQLite amalgamation (public domain), the Ubuntu Light font
(Ubuntu Font License 1.0), an Oodle1 decompressor port and Granny curve decoding from nwn2mdk (Apache-2.0,
and Boost 1.0 for the Oodle1 file; see `third_party/NOTICE-granny.md`), a port of the FLIP image metric (BSD-3-Clause, NVIDIA), the egui default
fonts, and Rust crates from crates.io, each under its own license.

## Trademarks

Unofficial fan project, not affiliated with, endorsed, sponsored or approved by Firaxis Games, 2K, Aspyr or
Take-Two Interactive. Sid Meier's Civilization V, Civ, Civilization, 2K Games, Firaxis Games, Take-Two
Interactive Software and their logos are trademarks of Take-Two Interactive Software, Inc. Lekmod is the work
of its own authors. "Civilization" is used here only to describe compatibility.

## Contact

If you hold rights in something you believe should not be in this repository, open an issue or a private
report as described in [SECURITY.md](../SECURITY.md), and we will look at it promptly.
