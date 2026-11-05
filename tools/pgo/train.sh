#!/usr/bin/env bash
# Training run for the profile-guided Linux server core (tools/pgo/README.md).
#
#   tools/pgo/train.sh [--out FILE] [--work DIR] [--jobs N] [--save FILE:TURNS ...]
#
# 1. builds the instrumented core (build.sh --pgo-gen) from the fork ($LEKMOD_ROOT, the checkout the server core comes from),
# 2. builds the `live_hang` test of game-host against it (the harness that plays the AI phase of a saved game on the real core),
# 3. plays every training save for TURNS turns (-> .profraw files),
# 4. merges them into FILE (default: $LEKMOD_ROOT/headless/tools/clang/pgo/gamecore-linux.profdata) and writes FILE.txt (what it was
#    made from).
#
# Run it in Linux/WSL (clang 18 with llvm-profdata-18, the libclang_rt.profile archive, a Rust toolchain). Data stays under --work
# (default ~/pgo-train, about 3 GB with the Rust target dir; delete it afterwards).
#
# Training set. The profile must see what the server runs: the AI phase of a big late game, and the early game (settlers, workers,
# the first explorers). Default: PGO_SAVES, else the two saves below, which are the ones the committed profile was made from:
#   <workspace>/saves/game936-2026-10-31/936/ring/00000235-t0126.c5sv:2       turns 126-127, Huge 12-major game, Emperor, Online speed
#   <workspace>/assets-out/saves/late-huge/state/games/1/keep/00000031-t0030.c5sv:3   turns 30-32 of the late-huge test game
# (saves are not in git: pass your own with --save, or set PGO_SAVES="file:turns file:turns").
#
# Refresh the profile when the fork changes the AI or the pathfinding code (a stale profile is still correct, only less fast), and
# commit the new file in the fork. The gate that PGO changes nothing but speed: tools/pgo/README.md.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# The workspace (holds saves/ and assets-out/): PGO_WORKSPACE, else the nearest parent of the checkout with assets-out/gamedata-b2
# (the main checkout sits in it, a worktree one level lower).
WORKSPACE="${PGO_WORKSPACE:-}"
if [ -z "$WORKSPACE" ]; then
	WORKSPACE="$ROOT"
	while [ "$WORKSPACE" != / ] && [ ! -d "$WORKSPACE/assets-out/gamedata-b2" ]; do WORKSPACE="$(dirname "$WORKSPACE")"; done
fi
: "${LEKMOD_ROOT:?set LEKMOD_ROOT to the fork checkout the core is built from}"
LEKMOD_ROOT="$(cd "$LEKMOD_ROOT" && pwd)"

OUT="$LEKMOD_ROOT/headless/tools/clang/pgo/gamecore-linux.profdata"
WORK="${PGO_WORK:-$HOME/pgo-train}"
JOBS="$(nproc 2>/dev/null || echo 4)"
SAVES=()
while [ $# -gt 0 ]; do
	case "$1" in
		--out) OUT="$2"; shift 2 ;;
		--work) WORK="$2"; shift 2 ;;
		--jobs|-j) JOBS="$2"; shift 2 ;;
		--save) SAVES+=("$2"); shift 2 ;;
		-h|--help) sed -n '2,24p' "$0"; exit 0 ;;
		*) echo "train.sh: unknown argument '$1'" >&2; exit 2 ;;
	esac
done
if [ ${#SAVES[@]} -eq 0 ] && [ -n "${PGO_SAVES:-}" ]; then read -r -a SAVES <<< "$PGO_SAVES"; fi
if [ ${#SAVES[@]} -eq 0 ]; then
	SAVES=("$WORKSPACE/saves/game936-2026-10-31/936/ring/00000235-t0126.c5sv:2"
		"$WORKSPACE/assets-out/saves/late-huge/state/games/1/keep/00000031-t0030.c5sv:3")
fi
GAMEDATA="${CIV5_GAMEDATA:-$WORKSPACE/assets-out/gamedata-b2}"
[ -d "$GAMEDATA" ] || { echo "train.sh: game data not found: $GAMEDATA (CIV5_GAMEDATA)" >&2; exit 2; }
for s in "${SAVES[@]}"; do [ -f "${s%:*}" ] || { echo "train.sh: save not found: ${s%:*}" >&2; exit 2; }; done

CXX="${CXX:-$(command -v clang++-18 || command -v clang++)}"
PROFDATA="${LLVM_PROFDATA:-$(command -v llvm-profdata-18 || command -v llvm-profdata)}"
RT="$("$CXX" --print-file-name=libclang_rt.profile-x86_64.a)"
[ -f "$RT" ] || { echo "train.sh: libclang_rt.profile-x86_64.a not found (apt install libclang-rt-18-dev)" >&2; exit 2; }
export PATH="$HOME/.cargo/bin:$PATH"

mkdir -p "$WORK/prof"
rm -f "$WORK"/prof/*.profraw

echo "train.sh: [1/4] instrumented core"
GIT_COMMIT="${GIT_COMMIT:-$(git -C "$LEKMOD_ROOT" rev-parse HEAD 2>/dev/null || echo unknown)}" \
	bash "$LEKMOD_ROOT/headless/tools/clang/build.sh" --mode lib --target linux --pgo-gen --out "$WORK/gc-gen" --jobs "$JOBS" >"$WORK/build-core.log" 2>&1 \
	|| { tail -20 "$WORK/build-core.log"; exit 1; }

echo "train.sh: [2/4] live_hang test against it"
# LLVM_PROFILE_FILE: the build scripts link the profile runtime too and would leave default.profraw files in the checkout
EXE="$(cd "$ROOT" && LLVM_PROFILE_FILE="$WORK/build-%p.profraw" GAMECORE_LIB_DIR_x86_64="$WORK/gc-gen" CARGO_TARGET_DIR="$WORK/target" CARGO_INCREMENTAL=0 \
	RUSTFLAGS="-C link-arg=-Wl,-u,__llvm_profile_runtime -C link-arg=$RT" \
	cargo test --release -p game-host --test live_hang --no-run --message-format=json 2>"$WORK/build-test.log" \
	| grep '"executable"' | grep live_hang | sed 's/.*"executable":"\([^"]*\)".*/\1/' | tail -1)"
[ -n "$EXE" ] && [ -x "$EXE" ] || { tail -20 "$WORK/build-test.log"; echo "train.sh: the live_hang test did not build" >&2; exit 1; }

echo "train.sh: [3/4] training runs"
for s in "${SAVES[@]}"; do
	f="${s%:*}"; t="${s##*:}"
	echo "  $f ($t turns)"
	(cd "$ROOT" && LLVM_PROFILE_FILE="$WORK/prof/%p.profraw" CIV5_GAMEDATA="$GAMEDATA" CIV5_LIVE_SAVE="$f" CIV5_LIVE_TURNS="$t" \
		"$EXE" --nocapture --test-threads=1 2>&1 | grep -E '^TURN|test result|panicked' | sed 's/^/    /')
done

echo "train.sh: [4/4] merge"
mkdir -p "$(dirname "$OUT")"
"$PROFDATA" merge --sparse -o "$OUT" "$WORK"/prof/*.profraw
{
	echo "gamecore PGO profile (llvm-profdata merge --sparse of the training runs of tools/pgo/train.sh)"
	echo "fork commit: ${GIT_COMMIT:-unknown}"
	echo "compiler: $("$CXX" --version | head -n1)"
	echo "date: $(date -u +%Y-%m-%d)"
	echo "training:"
	for s in "${SAVES[@]}"; do echo "  $(basename "${s%:*}") x ${s##*:} turns"; done
	echo "sha1: $(sha1sum "$OUT" | cut -c1-40)"
} >"$OUT.txt"
ls -l "$OUT" "$OUT.txt"
echo "train.sh: done. Commit $OUT (and .txt) in the fork; remove $WORK when you do not need it."
