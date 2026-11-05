# Contributing

Thanks for helping. Issues with clear steps to reproduce, and focused pull requests, are the most useful.

## Set up

Follow [docs/install.md](docs/install.md) for the toolchains and [docs/building.md](docs/building.md) for the
commands. Most work needs only:

```sh
cargo build --workspace --locked
cargo xtask test
cargo xtask dev            # local server with the fixture test world on http://127.0.0.1:8080/
```

Changes that touch the gamecore, the replica or the original UI should also be tried with the real gamecore
(`cargo xtask gamecore`, then `--gamecore-lib target/gamecore`) and your own asset pack.

## Before you open a pull request

- `cargo xtask check` passes: rustfmt, clippy with `-D warnings` for native and wasm32, the crate dependency
  rules, the game-file check, the gateway rule and the wasm import check.
- `cargo xtask test` passes; new behaviour has tests. Tests must not need game files unless they skip
  cleanly when the variables that point at them (`CIV5_ROOT`, `CIV5_DB_SNAPSHOT`, ...) are not set; use small
  synthetic fixtures made by the test.
- One topic per pull request. Say what changed and why, and how you checked it (commands, browser, OS, GPU).
- Code, comments and messages are in English.
- If a change knowingly behaves differently from the original game, mark it in the code with a
  `SIMPLIFIED(...)` comment that says what differs. The original UI layer keeps its list in
  `crates/client/exe-api/src/simp.rs`. The `SIMP-...` ids in these comments are internal tracking ids;
  their full register is kept outside this repository, so the comment itself must say enough.
- A change of the gamecore C ABI (`crates/shared/gamecore-sys/cpp/include/gamecore_abi.h`) needs the same
  change in the fork ([nikitagdi/civ5-coregame-reimagened](https://github.com/nikitagdi/civ5-coregame-reimagened))
  so that both copies stay byte-identical.

## Never commit game files

No files from the game or derived from it: textures, models, sounds, music, UI files, maps, saves, the game
database, pipeline output, or screenshots and videos of game content. The check in `cargo xtask check`
(`forbidden_game_files`) rejects `.exe`, `.dll`, `.dds`, `.gr2`, `.fpk`, `.civ5save`, `.db`, `.fxsxml`, `.wav`
and `.ogg` outside `third_party/`; it cannot catch everything, so review what you add. Keep the pipeline
output in `../assets-out`. Do not commit local paths, host names, passwords or tokens either:
`deploy/targets/*.toml` is git-ignored for that reason.

## License of contributions

By contributing you agree that your contribution is licensed under the license of the files it changes:
AGPL-3.0-or-later with the section 7 permission in [LICENSE-EXCEPTION.md](LICENSE-EXCEPTION.md), or MIT for
the crates whose `Cargo.toml` says so. See [docs/legal.md](docs/legal.md).

## Security

Do not report vulnerabilities in public issues; see [SECURITY.md](SECURITY.md).
