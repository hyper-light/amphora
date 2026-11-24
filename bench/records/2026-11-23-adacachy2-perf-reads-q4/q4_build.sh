#!/usr/bin/env bash
# Lane Q4: amphora-bench (release, bench's own profile as Q3 built it) at a ref, in the lane's one
# worktree and target dir (RULES 5c), copied to the lane's binary folder by full SHA.
# usage: JOBS=8 q4_build.sh <ref>
set -uo pipefail
export GIT_TERMINAL_PROMPT=0
R=~/.cache/amphora-perf/analytics-remote W=~/.cache/analytics-ab/w BIN=~/.cache/amphora-perf-bin/analytics L=~/.cache/analytics-q4
J=${JOBS:-8}
mkdir -p "$BIN" "$L"
export CARGO_HOME=$HOME/.cache/cargo-home-perf-analytics CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/analytics CARGO_BUILD_JOBS=$J CARGO_INCREMENTAL=0
state() { echo "load $(cut -d' ' -f1-3 /proc/loadavg) cpu_psi $(head -1 /proc/pressure/cpu | cut -d' ' -f2) io_psi $(head -1 /proc/pressure/io | cut -d' ' -f2)"; }
git -C "$R" fetch -q origin || exit 1
full=$(git -C "$R" rev-parse "$1") || exit 1
git -C "$W" checkout -q --detach "$full" || exit 1
cd "$W" || exit 1
s=$(date +%s)
echo "=== build $full $(date -u +%H:%M:%SZ) $(state)"
if [ ! -x "$BIN/amphora-bench-$full" ]; then
  (cd bench && nice -n 10 cargo build --release --bin amphora-bench) > "$L/build-$full.log" 2>&1 || { tail -30 "$L/build-$full.log"; exit 1; }
  cp "$CARGO_TARGET_DIR/release/amphora-bench" "$BIN/amphora-bench-$full" || exit 1
fi
echo "built $BIN/amphora-bench-$full in $(( $(date +%s) - s )) s $(state)"
