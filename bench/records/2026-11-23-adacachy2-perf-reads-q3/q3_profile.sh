#!/usr/bin/env bash
# Lane Q3: where point reads spend their CPU on one binary: a fresh 200,000-item load into the lane's
# own folder, then the five point statements profiled (AMPHORA_PROFILE, 997 Hz) at $CLIENTS.
# usage: BIN=<amphora-bench> CLIENTS=1 TAG=<name> q3_profile.sh
set -uo pipefail
L=~/.cache/analytics-q3 DATA=~/.cache/analytics-q3/data TAG=${TAG:-main} CLIENTS=${CLIENTS:-1}
ONLY='point_pk$,point_secondary$,point_text$,json_field$,blob_read$'
mkdir -p "$L" "$DATA"
export XDG_CACHE_HOME=$L/xdg
state() { echo "load $(cut -d' ' -f1-3 /proc/loadavg) cpu_psi $(head -1 /proc/pressure/cpu | cut -d' ' -f2) io_psi $(head -1 /proc/pressure/io | cut -d' ' -f2)"; }
reuse=--reuse; [ -e "$DATA/amphora/suite.db" ] || reuse=
echo "=== profile $TAG clients $CLIENTS ${reuse:-fresh load} $(date -u +%H:%M:%SZ) $(state)"
AMPHORA_PROFILE=$L/prof-$TAG nice -n 10 "$BIN" --suite --items 200000 --target amphora --dir "$DATA/amphora" --clients "$CLIENTS" --seconds 1 --warmup 0.5 --only "$ONLY" $reuse 2>"$L/prof-$TAG.err" | tail -1 | python3 -I -c '
import json, sys
r = json.loads(sys.stdin.read())
for x in r.get("results", []):
    ms = x.get("ms") or {}
    print("  ", x["op"], "clients", x["clients"], "ops/s", round(x.get("ops_per_s") or 0), "p50", ms.get("p50"), "p99", ms.get("p99"), "cpu_user_s", x.get("cpu_user_s"), "cpu_sys_s", x.get("cpu_sys_s"), "errors", x.get("errors"), "rows/op", x.get("rows_per_op"))'
for op in point_pk point_secondary point_text json_field blob_read; do
  for c in ${CLIENTS//,/ }; do
    f=$L/prof-$TAG-amphora-$op-$c.txt
    echo "--- $op $c clients ($f)"
    sed -n '/== threads ==/,/^$/p' "$f" | head -6
    sed -n '/== all: samples/,/^$/p' "$f" | head -22
    sed -n '/== all: inclusive/,/^$/p' "$f" | head -40
  done
done
echo "=== end $(date -u +%H:%M:%SZ) $(state)"
