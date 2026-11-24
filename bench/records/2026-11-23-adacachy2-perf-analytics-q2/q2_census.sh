#!/usr/bin/env bash
# Lane Q2: build the bench at a SHA (the diag branch's census hooks), then run point_secondary and
# table_count (S8's order, 1/32/128 clients) on the Q1 database with AMPHORA_CACHE_CENSUS set: each
# reader's cache census before and after its first four counts. usage: q2_census.sh <sha> [only]
set -uo pipefail
export GIT_TERMINAL_PROMPT=0
SHA=$1 ONLY=${2:-point_secondary\$,table_count\$}
R=~/.cache/amphora-perf/analytics-remote W=~/.cache/analytics-ab/w BIN=~/.cache/amphora-perf-bin/analytics L=~/.cache/analytics-q2
mkdir -p "$BIN" "$L" ~/.cache/cargo-home-perf-analytics
for d in registry git bin; do [ -e ~/.cache/cargo-home-perf-analytics/$d ] || ln -s ~/.cargo/$d ~/.cache/cargo-home-perf-analytics/$d; done
export CARGO_HOME=$HOME/.cache/cargo-home-perf-analytics CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/analytics CARGO_BUILD_JOBS=${JOBS:-8} CARGO_INCREMENTAL=0
state() { echo "load $(cut -d' ' -f1-3 /proc/loadavg) cpu_psi $(head -1 /proc/pressure/cpu | cut -d' ' -f2) io_psi $(head -1 /proc/pressure/io | cut -d' ' -f2)"; }
git -C "$R" fetch -q origin || exit 1
full=$(git -C "$R" rev-parse "$SHA") || exit 1
if [ ! -x "$BIN/amphora-bench-$full" ]; then
  git -C "$W" checkout -q --detach "$full" || exit 1
  s=$(date +%s); echo "=== build $full $(date -u +%H:%M:%SZ) $(state)"
  (cd "$W/bench" && nice -n 10 cargo build --release --bin amphora-bench > "$L/build-$full.log" 2>&1) || { tail -40 "$L/build-$full.log"; exit 1; }
  cp "$CARGO_TARGET_DIR/release/amphora-bench" "$BIN/amphora-bench-$full" || exit 1
  echo "built in $(( $(date +%s) - s )) s $(state)"
fi
DATA=~/.cache/analytics-q1/data/amphora
ls -d "$DATA" >/dev/null 2>&1 && REUSE=--reuse || { REUSE=; DATA=$L/data/amphora; mkdir -p "$DATA"; }
C=$L/census-$full; rm -rf "$C"; mkdir -p "$C"
export XDG_CACHE_HOME=$L/xdg
echo "=== census $ONLY reuse=${REUSE:-no} $(date -u +%H:%M:%SZ) $(state)"
AMPHORA_CACHE_CENSUS=$C nice -n 10 "$BIN/amphora-bench-$full" --suite --items 200000 --target amphora --dir "$DATA" --clients 1,32,128 --seconds 1 --warmup 0.5 --only "$ONLY" $REUSE 2>"$L/census-$full.err" | python3 -I -c '
import json, sys
r = json.loads(sys.stdin.read().strip().splitlines()[-1])
for x in r.get("results", []):
    print("  ", x["op"], "clients", x["clients"], "ops/s", round(x.get("ops_per_s") or 0), "p50", (x.get("ms") or {}).get("p50"))
'
echo "=== end $(date -u +%H:%M:%SZ) $(state) files $(ls "$C" | wc -l)"
python3 -I "$W/bench/records/2026-11-23-adacachy2-perf-analytics-q2/q2_census.py" "$C" 2>&1 | head -120
