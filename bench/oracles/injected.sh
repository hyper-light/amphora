#!/usr/bin/env bash
# G2 step 3's test: a bug injected into Amphora (a patch from bench/oracles/injected/), the bench built
# with it, and one nightly run of the oracles on that build. Passes when the run finds the bug: a
# finding whose queries answer as SQLite's on the clean build (`--replay-check`) and not on the
# injected one, so it's the injected bug's and not one Amphora has anyway. The patch is applied for
# the build only and taken back out whatever happens; the build goes to a target directory of its
# own, first copied from a warm one (a reflink on btrfs: nothing written).
#
# usage: bench/oracles/injected.sh <patch> <clean bench binary> <out dir>
#   WARM_TARGET     a target dir the bench is built in (default ~/.cache/amphora-prof-target)
#   INJECT_TARGET   the injected build's (default ~/.cache/amphora-inject-target)
#   ORACLE_SECONDS  the run's budget (default 1800, a nightly run's)
#   ORACLE_SQLITE   the reference library (default: what bench/oracles/sqlite.sh builds)
set -uo pipefail
HERE=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
ROOT=$(git -C "$HERE" rev-parse --show-toplevel)
PATCH=$(realpath -- "$1")
CLEAN=$(realpath -- "$2")
OUT=$(realpath -m -- "$3")
T=${INJECT_TARGET:-$HOME/.cache/amphora-inject-target}
mkdir -p "$OUT"
[[ -d "$T" ]] || cp -a --reflink=auto "${WARM_TARGET:-$HOME/.cache/amphora-prof-target}" "$T" || exit 1
git -C "$ROOT" apply "$PATCH" || exit 1
trap 'git -C "$ROOT" apply -R "$PATCH"' EXIT
(cd "$ROOT/bench" && CARGO_TARGET_DIR=$T cargo build --release --locked) || exit 1
cp "$T/release/amphora-bench" "$OUT/amphora-bench-injected" || exit 1
git -C "$ROOT" apply -R "$PATCH" && trap - EXIT
SQLITE=${ORACLE_SQLITE:-$("$HERE/sqlite.sh")} || exit 1
ORACLE_SQLITE=$SQLITE "$HERE/nightly.sh" "$OUT/amphora-bench-injected" "$OUT/run"
caught=0
for f in "$OUT"/run/findings/*.md; do
  [[ -e "$f" ]] || continue
  if (cd "$ROOT" && "$CLEAN" --oracle --sqlite "$SQLITE" --replay-check "$f" >/dev/null) \
    && ! (cd "$ROOT" && "$OUT/amphora-bench-injected" --oracle --sqlite "$SQLITE" --replay-check "$f" >/dev/null); then
    echo "the injected bug's: $(basename "$f") ($(grep -m1 '^# ' "$f"))"
    caught=$((caught + 1))
  fi
done
if [[ $caught -gt 0 ]]; then echo "caught: $caught findings are the injected bug's ($(basename "$PATCH"))"; exit 0; fi
echo "missed: no finding of the nightly run is the injected bug's ($(basename "$PATCH"))"
exit 1
