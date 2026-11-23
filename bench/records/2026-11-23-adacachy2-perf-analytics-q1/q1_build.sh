#!/usr/bin/env bash
# Lane Q1: build the bench at each SHA given (in order) in one worktree and one target dir, each
# binary copied to ~/.cache/amphora-perf-bin/analytics/amphora-bench-<sha>.
set -uo pipefail
export GIT_TERMINAL_PROMPT=0
R=~/.cache/amphora-perf/analytics-remote W=~/.cache/analytics-ab/w BIN=~/.cache/amphora-perf-bin/analytics L=~/.cache/analytics-q1
mkdir -p "$BIN" "$L" ~/.cache/analytics-ab ~/.cache/cargo-home-perf-analytics
for d in registry git bin; do [ -e ~/.cache/cargo-home-perf-analytics/$d ] || ln -s ~/.cargo/$d ~/.cache/cargo-home-perf-analytics/$d; done
export CARGO_HOME=$HOME/.cache/cargo-home-perf-analytics CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/analytics CARGO_BUILD_JOBS=${JOBS:-8} CARGO_INCREMENTAL=0
state() { echo "load $(cut -d' ' -f1-3 /proc/loadavg) cpu_psi $(head -1 /proc/pressure/cpu | cut -d' ' -f2) io_psi $(head -1 /proc/pressure/io | cut -d' ' -f2)"; }
if [ ! -d "$R/.git" ]; then
  ref=$(ls -d ~/.cache/amphora-perf/*-remote/.git 2>/dev/null | grep -v analytics | head -1)
  git clone -q ${ref:+--reference-if-able "${ref%/.git}"} git@github.com:hyper-light/amphora.git "$R" || exit 1
fi
git -C "$R" fetch -q origin || exit 1
[ -d "$W/.git" ] || [ -f "$W/.git" ] || git -C "$R" worktree add -q --detach "$W" origin/main || exit 1
for sha in "$@"; do
  full=$(git -C "$R" rev-parse "$sha") || exit 1
  [ -x "$BIN/amphora-bench-$full" ] && { echo "have $full"; continue; }
  git -C "$W" checkout -q --detach "$full" || exit 1
  s=$(date +%s)
  echo "=== build $full $(date -u +%H:%M:%SZ) $(state)"
  (cd "$W/bench" && nice -n 10 cargo build --release --bin amphora-bench > "$L/build-$full.log" 2>&1) || { tail -30 "$L/build-$full.log"; exit 1; }
  cp "$CARGO_TARGET_DIR/release/amphora-bench" "$BIN/amphora-bench-$full" || exit 1
  echo "built $full in $(( $(date +%s) - s )) s $(state)"
done
