#!/usr/bin/env bash
# Lane Q3: the merge gate at a SHA, in the lane's one worktree and target dir (RULES 5c): the bench
# built (release) and kept for the A/B, the touched modules' tests (storage::log_segments,
# storage::wal), clippy on amphora-engine (all targets), then scripts/check-production.sh.
# usage: JOBS=4 q3_check.sh <sha>
set -uo pipefail
export GIT_TERMINAL_PROMPT=0
R=~/.cache/amphora-perf/analytics-remote W=~/.cache/analytics-ab/w BIN=~/.cache/amphora-perf-bin/analytics L=~/.cache/analytics-q3
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
echo "built in $(( $(date +%s) - s )) s $(state)"
echo "=== tests $(date -u +%H:%M:%SZ) $(state)"
nice -n 10 cargo test -p amphora-engine --lib --locked -- storage::log_segments storage::wal --test-threads="$J" > "$L/test-$full.log" 2>&1
echo "tests exit $?"; grep -E "^test result|FAILED|panicked" "$L/test-$full.log" | head -20
echo "=== clippy $(date -u +%H:%M:%SZ) $(state)"
nice -n 10 cargo clippy -p amphora-engine --all-targets --locked -- -D warnings > "$L/clippy-$full.log" 2>&1
echo "clippy exit $?"; grep -E "^(warning|error)" -A 8 "$L/clippy-$full.log" | head -40
echo "=== check-production $(date -u +%H:%M:%SZ) $(state)"
nice -n 10 bash scripts/check-production.sh > "$L/checkprod-$full.log" 2>&1
echo "check-production exit $?"; tail -12 "$L/checkprod-$full.log"
echo "=== end $(date -u +%H:%M:%SZ) $(state)"
