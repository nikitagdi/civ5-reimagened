# Profile-guided Linux server core (PGO)

The AI phase of a late Huge game is single-core CPU work. Building the Linux core with
clang profile-guided optimisation (`-fprofile-instr-use`) makes it about 12 % faster on turns 126-127 of ring save 235 of game 936.
The wasm core is not profile-guided (the replica does not run the AI).

* **Switch.** `cargo xtask gamecore --pgo`, `cargo xtask dist --pgo`, `cargo xtask deploy --target <target> --pgo`. Off by default; without
  the switch the build is what it was. The switch passes `--pgo` to the fork's `headless/tools/clang/build.sh` for the Linux archive
  only (`--target linux`); the profile is the one committed in the fork,
  `headless/tools/clang/pgo/gamecore-linux.profdata` (`$LEKMOD_ROOT`), made from the OPT set of the portable build, OPT-0086 included.
* **Build id.** Unchanged, by design. The id covers the FIX / HYG defines; OPT defines and compiler options are outside it, because
  they change no result (an OPT is proven by equal digests and saves; PGO is proven the same way below). So a save of a PGO core
  loads in a plain core and back, a PGO server serves replicas of the plain wasm core, and `deploy` does not call it a core change.
  `build-info.txt` of the archive has the line `pgo: use <sha1 of the profile>`.
* **Refresh.** `LEKMOD_ROOT=<fork> tools/pgo/train.sh` in Linux/WSL (about 6 min on a quiet 24-thread machine, 3 GB of scratch under
  `~/pgo-train`): instrumented core (`build.sh --pgo-gen`), `live_hang` of `game-host` on the training saves, `llvm-profdata merge
  --sparse`. Training set of the committed profile: turns 126-127 of ring save 235 (the AI phase of the live game) and turns 30-32
  of the `late-huge` test game (early game). Commit the new profile in the fork whenever the fork changes the AI or pathfinding code.
  A stale profile is still correct (clang drops the profile of a function whose control flow changed), only less fast.
* **Gate.** `LEKMOD_ROOT=<fork> tools/pgo/compare.sh`: builds the plain and the PGO core, plays six saves (turns the profile was not
  trained on among them) with `live_hang`, and compares the digest and the xxh64 of the GCS1 part of the save of every turn start;
  exit 1 on any difference. Run it after every refresh and before a deploy with `--pgo`.
