# Building

Toolchains are listed in [install.md](install.md#toolchains). All commands run from the repository root.
`cargo xtask` is an alias (`.cargo/config.toml`) for `cargo run -p xtask --`; running it without a command
prints the full usage line.

## Folder layout

```text
<work>/
  civ5-reimagened/      this repository
  Lekmod/               the gamecore fork (LEKMOD_ROOT overrides the location)
  toolchains/wasi-sdk/  optional default location of wasi-sdk 33 (WASI_SDK overrides it)
  assets-out/           asset pipeline output (default of --out), never committed
```

## Plain cargo

```sh
cargo build --workspace --locked
cargo test --workspace --locked
```

This builds every native crate with the C++ stub gamecore (`crates/shared/gamecore-sys/cpp/stub`). The stub
and the fixture test world are enough for most client, protocol and tooling work.

## cargo xtask commands

| Command | What it does |
|---|---|
| `build [--only web\|native] [--release] [--single]` | native: server and desktop app; web: wasm-bindgen bundle in `target/web` (`pkg`, plus the threaded `pkg-mt` unless `--single`). No arguments: both |
| `check` | `cargo fmt --check`, clippy native and wasm32 with `-D warnings`, dependency rules, game files in the git index, the gateway has no gamecore outside the worker, ABI header equal to the fork's, allowed wasm imports |
| `deps` | the crate dependency rules alone (`tools/xtask/deps.toml`) |
| `test [cargo test args]` | `cargo test --workspace` unless you pick packages with `-p` |
| `dev [--wsl] [--core fixture\|real] [--assets <dir>] [--gamedata-dir <dir>] [--state-dir <dir>] [--listen <addr>]` | builds the web client and runs `civ5-server --dev` on 127.0.0.1:8080; fixture core unless `--wsl` |
| `run-desktop -- <args>` | the native client (`apps/desktop`); it links the stub core |
| `assets [--game <dir>] [--out <dir>] [--set <name>] [--db <dir>]` | one asset-pipeline set; for several sets call the pipeline directly (see [install.md](install.md)) |
| `gamecore [--out <dir>] [--target linux\|wasm\|wasm-mt\|all] [--pgo]` | builds the real gamecore archives from the fork into `target/gamecore/<target>` |
| `dist [--skip-gamecore] [--pgo]` | `target/dist/civ5-<build>/`: Linux release server, release web bundle, `Dockerfile`, `server.toml` |
| `deploy --target <file.toml\|name> [--dry-run] [--rollback] ...` | builds, syncs and starts a host; see [hosting.md](hosting.md) |
| `playtest [--scenario ...] [--headed] ...` | browser playtests with Playwright (`tools/playtest`, Node.js 22+) |
| `bench [--compare <baseline.json>] [--no-compare]` | the render benchmark scene in a headed browser; see [performance.md](performance.md) |
| `web-selftest [--release] [--scenario tiny\|small\|late-huge\|all]` | replica self-tests in the browser |
| `nettest [--scenario ...] [--quick\|--full] [--wsl] [--link none\|proxy\|netem]` | network test stand with headless bots (`tools/net-test`) |
| `exe-api-coverage --contexts <...>` | coverage report of the globals the original UI's Lua uses |
| `golden ...`, `shader-re ...` | developer tools that compare our frames and shaders with the original game; they need captures and files from your own install |
| `wasm-imports <file.wasm>` | lists the import modules of a wasm file |

`build`, `check`, `test`, `dev`, `playtest` and `web-selftest` accept `--gamecore-lib <dir>`: the real
archives in `<dir>/linux`, `<dir>/wasm` and `<dir>/wasm-mt` are linked instead of the stub. On Windows only the
wasm archives are linked natively; the Linux one is used inside WSL.

## The gamecore

The gamecore comes from the fork
[nikitagdi/Lekmod](https://github.com/nikitagdi/Lekmod/tree/reimagened/v34.15), branch
`reimagened/v34.15` (Lekmod v34.15 plus the `headless/` portable build). Its `headless/README.md` describes
the build in full.

```sh
cargo xtask gamecore                 # Linux: linux, wasm, wasm-mt.  Windows (via WSL): wasm, wasm-mt
cargo xtask gamecore --target all    # all three on either OS
cargo xtask gamecore --pgo           # the Linux archive profile-guided (profile committed in the fork)
LEKMOD_ROOT=/path/to/Lekmod cargo xtask gamecore
```

It runs the fork's `headless/tools/clang/build.sh --mode lib`. Requirements (as tested): Ubuntu 24.04,
clang 18.1.3 (`clang++-18`, `llvm-ar`), GNU make, bash, python3 3.10+, and wasi-sdk 33 for the wasm targets
(`WASI_SDK`, inside WSL default `~/wasi-sdk-33`). On Windows it runs inside WSL on `/mnt/<drive>` paths.
Each output folder holds `libgamecore.a` and `build-info.txt`.

The ABI header `crates/shared/gamecore-sys/cpp/include/gamecore_abi.h` must be byte-identical to the fork's
`headless/portable/abi/gamecore_abi.h`; `cargo xtask check` compares them when it finds the fork and prints a
notice otherwise. A header change goes into both repositories, plus `gamecore-sys/src/ffi.rs` and the stub.

The PGO profile is made with `tools/pgo/train.sh` (see [../tools/pgo/README.md](../tools/pgo/README.md)).
PGO changes code layout only; saves and the build id stay the same.

## Web client

`cargo xtask build --only web` writes `target/web/`: the page files from `apps/web/www`, `pkg/` (one thread)
and `pkg-mt/` (threads: std rebuilt with atomics through `-Z build-std` with `RUSTC_BOOTSTRAP=1` on the
`stable` toolchain). The page takes `pkg-mt` when it is cross-origin isolated (COOP/COEP headers, see
[hosting.md](hosting.md)), else `pkg`. `--release` also runs `wasm-opt -O`.

## Environment variables

| Variable | Used by | Meaning |
|---|---|---|
| `LEKMOD_ROOT` | xtask, pipeline | the gamecore fork checkout (default `../Lekmod`) |
| `WASI_SDK` | wasm builds | wasi-sdk 33 root (default `../toolchains/wasi-sdk`) |
| `CIV5_WSL_DISTRO` | xtask on Windows | WSL distribution (default `Ubuntu-24.04`) |
| `CIV5_MT_TOOLCHAIN` | threaded web build | toolchain with `rust-src` (default `stable`) |
| `CIV5_WASM_OPT`, `CIV5_SKIP_WASM_OPT` | release web build | path of `wasm-opt`; skip it |
| `GAMECORE_LIB_DIR_x86_64`, `GAMECORE_LIB_DIR_wasm32`, `GAMECORE_LIB_DIR_wasm32_threads` | `gamecore-sys` | archive folders, the long form of `--gamecore-lib` |
| `CIV5_ROOT` | asset pipeline | the game folder (instead of Steam detection) |
| `CIV5_DB_SNAPSHOT` | asset pipeline | the DB snapshot folder (instead of `--db`) |
| `CIV5_GAMEDATA_STRICT` | asset pipeline | `1`: a snapshot that differs from the lock is an error |
| `LEKMOD_REPO` | `export-gamedata` | the fork, for the Lua fix (else `LEKMOD_ROOT`) |
| `CIV5_PASSWORD` | server, deploy | the registration (invite) code, at least 8 characters |

## Tests with real data

Most tests use synthetic fixtures. Tests that need your install or the real gamecore skip unless their
variables are set (`CIV5_ROOT`, `CIV5_DB_SNAPSHOT`, `CIV5_GAME_DIR`, `--gamecore-lib`).
