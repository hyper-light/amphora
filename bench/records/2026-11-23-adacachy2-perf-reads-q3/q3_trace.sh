#!/usr/bin/env bash
# Lane Q3: where point-read clients wait at 128 clients: every thread's state, syscall and wchan sampled
# from /proc (Q1's q1_sample.py, no profiler in that run), then the same statements profiled
# (AMPHORA_PROFILE) at 32 and 128 clients, on the lane's own database (q3_profile.sh loads it).
# usage: BIN=<amphora-bench> TAG=<name> q3_trace.sh
set -uo pipefail
L=~/.cache/analytics-q3 DATA=~/.cache/analytics-q3/data TAG=${TAG:-main}
ONLY='point_pk$,point_secondary$,point_text$,json_field$,blob_read$'
export XDG_CACHE_HOME=$L/xdg
state() { echo "load $(cut -d' ' -f1-3 /proc/loadavg) cpu_psi $(head -1 /proc/pressure/cpu | cut -d' ' -f2) io_psi $(head -1 /proc/pressure/io | cut -d' ' -f2)"; }
echo "=== trace $TAG 128 clients $(date -u +%H:%M:%SZ) $(state)"
python3 -I ~/.cache/analytics-q1/q1_sample.py "$L/trace-$TAG.json" nice -n 10 "$BIN" --suite --items 200000 --target amphora --dir "$DATA/amphora" --clients 128 --seconds 1 --warmup 0.5 --only "$ONLY" --reuse 2>"$L/trace-$TAG.err" | cut -c1-1500
echo "=== profile $TAG 32,128 clients $(date -u +%H:%M:%SZ) $(state)"
AMPHORA_PROFILE=$L/prof-$TAG nice -n 10 "$BIN" --suite --items 200000 --target amphora --dir "$DATA/amphora" --clients 32,128 --seconds 1 --warmup 0.5 --only "$ONLY" --reuse 2>"$L/prof-$TAG-mc.err" | tail -1 | python3 -I -c '
import json, sys
r = json.loads(sys.stdin.read())
for x in r.get("results", []):
    ms = x.get("ms") or {}
    print("  ", x["op"], "clients", x["clients"], "ops/s", round(x.get("ops_per_s") or 0), "p50", ms.get("p50"), "p99", ms.get("p99"), "cpu_user_s", x.get("cpu_user_s"), "cpu_sys_s", x.get("cpu_sys_s"), "errors", x.get("errors"))'
for op in point_pk point_secondary point_text; do
  for c in 128; do
    f=$L/prof-$TAG-amphora-$op-$c.txt
    echo "--- $op $c clients ($f)"
    sed -n '/== all: samples/,/^$/p' "$f" | head -16
    sed -n '/== all: inclusive/,/^$/p' "$f" | head -34 | cut -c1-160
  done
done
echo "=== end $(date -u +%H:%M:%SZ) $(state)"
