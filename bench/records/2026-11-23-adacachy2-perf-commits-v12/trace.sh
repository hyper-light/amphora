#!/usr/bin/env bash
# Lane V12: build diag/commits-v12-stages's bench and run trace.py at 1 writer (spread and leader)
# and 32 spread writers, both classes, then stages_table.py on each. usage: trace.sh OUT (repo root)
set -uo pipefail
OUT=${1:?usage: trace.sh OUT}
ROOT=$(git rev-parse --show-toplevel); S="$ROOT/bench/records/2026-11-23-adacachy2-perf-commits-v12"
W2="$ROOT/bench/records/2026-11-23-adacachy2-perf-oltp"
mkdir -p "$OUT"
cc -O2 -shared -fPIC -o "$OUT/stages.so" "$W2/stages.c" -ldl || exit 4
cargo build --release --manifest-path "$ROOT/bench/Cargo.toml" || exit 1
TARGET=$(cargo metadata --format-version 1 --no-deps --manifest-path "$ROOT/bench/Cargo.toml" | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
BIN=$HOME/.cache/amphora-perf-bin/commits/v12-stages-$(git rev-parse --short=8 HEAD); cp "$TARGET/release/amphora-bench" "$BIN" || exit 1
ulimit -n "$(ulimit -Hn)" 2>/dev/null || true
for cls in persisted replicated; do
  for wr in "1 spread" "1 leader" "32 spread"; do
    read -r w r <<<"$wr"; o="$OUT/$cls-$w-$r"
    echo "$(date -u +%T) $cls $w $r load=$(cut -d' ' -f1-3 /proc/loadavg) cpu=$(awk 'NR==1{print $2}' /proc/pressure/cpu) io=$(awk 'NR==1{print $2}' /proc/pressure/io)"
    nice -n 10 python3 -I "$S/trace.py" "$BIN" "$OUT/stages.so" 29490 "$o" "$w" "$r" "$cls"
  done
done
for o in "$OUT"/*-*-*/; do python3 -I "$S/stages_table.py" "${o%/}" || echo "table failed for $o"; done
