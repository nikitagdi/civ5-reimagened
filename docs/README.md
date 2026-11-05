# civ5-reimagened documentation

Short pages, one topic each. Start with the install page if you want to play; with building and
architecture if you want to change the code.

| Page | What it covers |
|---|---|
| [install.md](install.md) | What you need per OS, the game requirements, the DB snapshot, the asset pipeline, troubleshooting |
| [building.md](building.md) | Toolchains, every `cargo xtask` command, the gamecore build from the Lekmod fork, environment variables |
| [hosting.md](hosting.md) | Server configuration, ports, Docker Compose, TLS reverse proxy, accounts and invite codes, admin API |
| [architecture.md](architecture.md) | Crate layout, the server-authoritative model with a checked replica, the gamecore C ABI, asset pack, renderer |
| [performance.md](performance.md) | How we measure, the numbers with their conditions and baselines, what is not measured yet |
| [limitations.md](limitations.md) | Render gaps, features that do not exist, platform and browser status |
| [legal.md](legal.md) | Asset policy, what the converter does, trademarks, licenses |

Repository files: [../README.md](../README.md), [../CONTRIBUTING.md](../CONTRIBUTING.md),
[../SECURITY.md](../SECURITY.md), [../NOTICE](../NOTICE), [../LICENSE-EXCEPTION.md](../LICENSE-EXCEPTION.md).

Tool-specific notes live next to the tools: [../tools/asset-pipeline/README.md](../tools/asset-pipeline/README.md),
[../tools/pgo/README.md](../tools/pgo/README.md).

## Terms used in these pages

- **Gamecore**: the Lekmod build of the game's rules and AI (`CvGameCoreDLL_Expansion2`), compiled with clang
  into a static library `libgamecore.a` for Linux (server) and WebAssembly (browser replica).
- **Gateway**: the `civ5-server` process: HTTP, WebSocket, accounts, lobby, asset and snapshot serving.
- **Worker**: a child process of the gateway that runs one game's gamecore.
- **Replica**: the copy of the game in the browser; it follows the server and never decides anything.
- **Asset pack**: the content-addressed blobs and the manifest the asset pipeline writes from your install.
- **Game data**: the folder `export-gamedata` writes for the server's gamecore (game DB, Lua, maps).
- **Fixture core / test world**: a small built-in world used when no real gamecore is linked.
