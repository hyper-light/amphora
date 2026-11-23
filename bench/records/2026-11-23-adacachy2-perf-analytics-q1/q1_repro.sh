#!/usr/bin/env bash
# Lane Q1, step 2: S8's order (full scans before table_count) on a settled database: does table_count
# fall after table_group_by / filter_no_index? Profile and per-thread trace (read syscalls per phase).
set -uo pipefail
L=~/.cache/analytics-q1 BIN=${BIN:-$HOME/.cache/amphora-perf-bin/bench-s7/amphora-bench-795faa8377d79ffe4d82f3eb7cb19717453c3a9e} TAG=${TAG:-s7}
ONLY=${ONLY:-table_group_by\$,table_count\$}
export XDG_CACHE_HOME=$L/xdg
state() { echo "load $(cut -d' ' -f1-3 /proc/loadavg) cpu_psi $(head -1 /proc/pressure/cpu | cut -d' ' -f2)"; }
summ() { python3 -I -c '
import json, sys
r = json.loads(sys.stdin.read().strip().splitlines()[-1])
for x in r.get("results", []):
    ms = x.get("ms") or {}
    print("  ", r["target"], x["op"], "clients", x["clients"], "ops/s", round(x.get("ops_per_s") or 0), "p50", ms.get("p50"), "p99", ms.get("p99"), "cpu_user_s", x.get("cpu_user_s"), "cpu_sys_s", x.get("cpu_sys_s"))
print("   stats", json.dumps(r.get("stats"))[:600])
'; }
echo "=== trace $ONLY $(date -u +%H:%M:%SZ) $(state)"
python3 -I $L/q1_sample.py $L/trace2-$TAG.json nice -n 10 "$BIN" --suite --items 200000 --target amphora --dir $L/data/amphora --clients 1,32,128 --seconds 1 --warmup 0.5 --only "$ONLY" --reuse 2>/dev/null
echo "=== profile $ONLY $(date -u +%H:%M:%SZ) $(state)"
AMPHORA_PROFILE=$L/prof2-$TAG nice -n 10 "$BIN" --suite --items 200000 --target amphora --dir $L/data/amphora --clients 1,32,128 --seconds 1 --warmup 0.5 --only "$ONLY" --reuse 2>/dev/null | summ
for c in 1 32; do
  f=$L/prof2-$TAG-amphora-table_count-$c.txt
  echo "--- profile table_count $c clients"
  sed -n '/== all: samples/,/^$/p' "$f" | head -22
  sed -n '/== all: inclusive/,/^$/p' "$f" | head -40
done
echo "=== turso, sqlite same order $(date -u +%H:%M:%SZ) $(state)"
for t in turso sqlite; do nice -n 10 "$BIN" --suite --items 200000 --target $t --dir $L/data/$t --clients 1,32,128 --seconds 1 --warmup 0.5 --only "$ONLY" --reuse 2>/dev/null | summ; done
echo "=== end $(date -u +%H:%M:%SZ) $(state)"
