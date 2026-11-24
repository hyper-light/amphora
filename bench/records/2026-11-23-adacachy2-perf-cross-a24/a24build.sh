#!/usr/bin/env bash
# Lane A24 on adacachy2, held to $JOBS cores, in the lane worktree of cross-remote: each SHA lib tests of
# amphora, one after the other in one target dir, copied to a24-tests-<sha>. usage: a24build.sh BRANCH:SHA...
set -uo pipefail
T=${JOBS:-4}
R=$HOME/.cache/amphora-perf/cross-remote
W=$HOME/.cache/amphora-perf/cross-a24-wt
for p in "$@"; do git -C "$R" cat-file -e "${p#*:}^{commit}" 2>/dev/null || git -C "$R" fetch -q origin "${p%%:*}" || exit 3; done
[ -d "$W" ] || git -C "$R" worktree add -q --detach "$W" "${1#*:}" || exit 3
cd "$W" || exit 2
export CARGO_HOME=$HOME/.cache/cargo-home-perf-cross PATH=$HOME/.cache/cargo-home-perf-cross/bin:$PATH CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/cross CARGO_BUILD_JOBS=$T CARGO_INCREMENTAL=0
B=$HOME/.cache/amphora-perf-bin/cross; mkdir -p "$B"
for p in "$@"; do
  s=${p#*:}
  git checkout -q -f --detach "$s" || exit 3
  [ "$(git rev-parse HEAD)" = "$s" ] || { echo "HEAD is not $s"; exit 1; }
  git log --oneline -1
  t0=$(date +%s)
  out=$(nice -n 10 cargo test -p amphora --lib --locked --no-run 2>&1)
  bin=$(echo "$out" | sed -n "s/.*Executable unittests src\/lib.rs (\(.*\))/\1/p")
  echo "$out" | grep -E "Compiling amphora(-engine)? |^error" | head -4
  echo "[$(date -u +%H:%M:%SZ)] built $s in $(( $(date +%s) - t0 ))s load=$(cut -d" " -f1-3 /proc/loadavg)"
  [ -x "$bin" ] || { echo "$out" | grep -E "^(error|warning)" -A 14 | head -60; exit 4; }
  cp "$bin" "$B/a24-tests-$s" || exit 5
done
ls -la "$B"/a24-tests-*
