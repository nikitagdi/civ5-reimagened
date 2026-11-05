#!/usr/bin/env bash
# The gate of a PGO profile: the profile-guided core and the plain core play the same saves; the digest of the state and the hash of
# the core's part of the save (GCS1) of every turn start must be equal, and the time of the turns is printed.
#
#   tools/pgo/compare.sh [--profile FILE] [--work DIR] [--jobs N] [--rounds N] [--save FILE:TURNS ...]
#
# Needs LEKMOD_ROOT (the fork checkout), Linux/WSL, the same tools as tools/pgo/train.sh. Exit code 1 when anything differs.
# Saves: --save, else PGO_SAVES, else the default evaluation set of train.sh plus the neighbours of the training saves
# (a profile is trained on one turn and used on every turn, so the gate must run turns it was not trained on).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
WORKSPACE="${PGO_WORKSPACE:-}"
if [ -z "$WORKSPACE" ]; then
	WORKSPACE="$ROOT"
	while [ "$WORKSPACE" != / ] && [ ! -d "$WORKSPACE/assets-out/gamedata-b2" ]; do WORKSPACE="$(dirname "$WORKSPACE")"; done
fi
: "${LEKMOD_ROOT:?set LEKMOD_ROOT to the fork checkout the core is built from}"
LEKMOD_ROOT="$(cd "$LEKMOD_ROOT" && pwd)"

PROFILE="$LEKMOD_ROOT/headless/tools/clang/pgo/gamecore-linux.profdata"
WORK="${PGO_WORK:-$HOME/pgo-compare}"
JOBS="$(nproc 2>/dev/null || echo 4)"
ROUNDS=3
SAVES=()
while [ $# -gt 0 ]; do
	case "$1" in
		--profile) PROFILE="$2"; shift 2 ;;
		--work) WORK="$2"; shift 2 ;;
		--jobs|-j) JOBS="$2"; shift 2 ;;
		--rounds) ROUNDS="$2"; shift 2 ;;
		--save) SAVES+=("$2"); shift 2 ;;
		-h|--help) sed -n '2,11p' "$0"; exit 0 ;;
		*) echo "compare.sh: unknown argument '$1'" >&2; exit 2 ;;
	esac
done
if [ ${#SAVES[@]} -eq 0 ] && [ -n "${PGO_SAVES:-}" ]; then read -r -a SAVES <<< "$PGO_SAVES"; fi
if [ ${#SAVES[@]} -eq 0 ]; then
	R="$WORKSPACE/saves/game936-2026-10-31/936/ring"
	L="$WORKSPACE/assets-out/saves/late-huge/state/games/1/keep"
	SAVES=("$R/00000235-t0126.c5sv:2" "$R/00000233-t0125.c5sv:3" "$R/00000236-t0126.c5sv:3"
		"$L/00000031-t0030.c5sv:3" "$L/00000091-t0090.c5sv:3" "$L/00000151-t0150.c5sv:3")
fi
GAMEDATA="${CIV5_GAMEDATA:-$WORKSPACE/assets-out/gamedata-b2}"
export PATH="$HOME/.cargo/bin:$PATH"
mkdir -p "$WORK"

build_core() { # name, build.sh flags...
	local name="$1"; shift
	GIT_COMMIT="${GIT_COMMIT:-$(git -C "$LEKMOD_ROOT" rev-parse HEAD 2>/dev/null || echo unknown)}" \
		bash "$LEKMOD_ROOT/headless/tools/clang/build.sh" --mode lib --target linux --out "$WORK/gc-$name" --jobs "$JOBS" "$@" >"$WORK/build-$name.log" 2>&1 \
		|| { tail -20 "$WORK/build-$name.log"; exit 1; }
}
build_test() { # name -> $WORK/lh-name
	local name="$1" exe
	exe="$(cd "$ROOT" && GAMECORE_LIB_DIR_x86_64="$WORK/gc-$name" CARGO_TARGET_DIR="$WORK/target" CARGO_INCREMENTAL=0 \
		cargo test --release -p game-host --test live_hang --no-run --message-format=json 2>"$WORK/test-$name.log" \
		| grep '"executable"' | grep live_hang | sed 's/.*"executable":"\([^"]*\)".*/\1/' | tail -1)"
	[ -n "$exe" ] && [ -x "$exe" ] || { tail -20 "$WORK/test-$name.log"; echo "compare.sh: live_hang did not build" >&2; exit 1; }
	cp "$exe" "$WORK/lh-$name"
}
play() { # name, save, turns -> "turn digest save" lines on stdout, seconds on fd 3 via the TURN lines
	(cd "$ROOT" && CIV5_GAMEDATA="$GAMEDATA" CIV5_LIVE_SAVE="$2" CIV5_LIVE_TURNS="$3" "$WORK/lh-$1" --nocapture --test-threads=1 2>&1 | grep -E '^TURN|panicked')
}

echo "compare.sh: plain core"; build_core plain
echo "compare.sh: profile-guided core ($PROFILE)"; build_core pgo --pgo-use "$PROFILE"
build_test plain; build_test pgo
grep -h '^pgo:' "$WORK/gc-pgo/build-info.txt" "$WORK/gc-plain/build-info.txt"
grep -h 'LEKMOD_PORTABLE_BUILD_TAG' "$WORK/gc-pgo/build-info.txt" "$WORK/gc-plain/build-info.txt" | sed 's/.*\(LEKMOD_PORTABLE_BUILD_TAG="[^"]*"\).*/build tag: \1/'

bad=0
for s in "${SAVES[@]}"; do
	f="${s%:*}"; t="${s##*:}"
	echo "== $(basename "$f") x $t turns"
	play plain "$f" "$t" | sed 's/ended in [0-9.]* s, [0-9]* steps;//' >"$WORK/ref.txt"
	play pgo "$f" "$t" | sed 's/ended in [0-9.]* s, [0-9]* steps;//' >"$WORK/new.txt"
	if cmp -s "$WORK/ref.txt" "$WORK/new.txt" && [ -s "$WORK/ref.txt" ]; then
		echo "  EQUAL: $(wc -l <"$WORK/ref.txt") turn starts, digest and save hash"; sed 's/^/    /' "$WORK/ref.txt"
	else
		echo "  DIFFERENT"; diff "$WORK/ref.txt" "$WORK/new.txt" || true; bad=1
	fi
done

echo "== time (turns of the first save, $ROUNDS interleaved rounds, plain / pgo)"
f="${SAVES[0]%:*}"; t="${SAVES[0]##*:}"
for i in $(seq "$ROUNDS"); do
	for n in plain pgo; do
		sum=$(play "$n" "$f" "$t" | awk '{for(i=1;i<=NF;i++) if($i=="in") s+=$(i+1)} END{printf "%.2f", s}')
		echo "  round $i $n: ${sum} s"
	done
done
[ "$bad" = 0 ] || { echo "compare.sh: DIFFERENCES FOUND"; exit 1; }
echo "compare.sh: all equal"
