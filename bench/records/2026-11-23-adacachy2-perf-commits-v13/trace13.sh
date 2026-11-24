#!/usr/bin/env bash
# Lane V13: build diag/commits-v13-refusals's bench, then trace13.py at 32 spread writers in both classes
# (ROUNDS runs each), then refusals.py on each. usage: trace13.sh OUT [ROUNDS] (repo root)
set -uo pipefail
OUT=${1:?usage: trace13.sh OUT [ROUNDS]}; ROUNDS=${2:-1}
ROOT=$(git rev-parse --show-toplevel); S="$ROOT/bench/records/2026-11-23-adacachy2-perf-commits-v13"
mkdir -p "$OUT"
cargo build --release --locked --manifest-path "$ROOT/bench/Cargo.toml" 2>&1 | tail -3 || exit 1
BIN=$HOME/.cache/amphora-perf-bin/commits/v13-trace-$(git rev-parse --short=8 HEAD); mkdir -p "$(dirname "$BIN")"
cp "$CARGO_TARGET_DIR/release/amphora-bench" "$BIN" || exit 1
ulimit -n "$(ulimit -Hn)" 2>/dev/null || true
for round in $(seq 1 "$ROUNDS"); do
  for cls in persisted replicated; do
    o="$OUT/$cls-32-spread-r$round"
    echo "$(date -u +%T) $cls 32 spread r$round load=$(cut -d' ' -f1-3 /proc/loadavg) cpu=$(awk 'NR==1{print $2}' /proc/pressure/cpu) io=$(awk 'NR==1{print $2}' /proc/pressure/io)"
    nice -n 10 python3 -I "$S/trace13.py" "$BIN" 29490 "$o" 32 spread "$cls"
    python3 -I "$S/refusals.py" "$o" || echo "refusals.py failed for $o"
  done
done
