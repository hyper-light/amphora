#!/usr/bin/env bash
# Lane T22 step 2: q10's plan A (star filters off) and plan B (by cost) in one binary (the trace branch 11c74723 on
# main ac2c429a), T17's S weights pinned, traced: the VDBE instruction sampled (AMPHORA_PC_TRACE), then the plan and seek
# traces in a process of their own (AMPHORA_PLAN_TRACE, AMPHORA_SEEK_TRACE), 5 timed runs each.
set -uo pipefail
BIN=${BIN:-~/.cache/amphora-perf-bin/tpch/amphora-bench-t22trace-11c74723} L=~/.cache/tpch-t22
T11=~/.cache/tpch-t11 XDG=$L/xdg-trace P=$L/S.json Q=$L/q/bench/planner/tpch/queries
mkdir -p "$L/traced" "$XDG" || exit 1
[ -d "$L/db-T" ] || cp -a --reflink=always "$T11/db-sf1" "$L/db-T" || exit 1
state() { echo "load $(cut -d' ' -f1-3 /proc/loadavg) cpu_psi $(head -1 /proc/pressure/cpu | cut -d' ' -f2) io_psi $(head -1 /proc/pressure/io | cut -d' ' -f2)"; }
b() { XDG_CACHE_HOME=$XDG nice -n 10 "$BIN" --planner --bench tpch --sf 1 --data ~/.cache/planner-bench/tpch-sf1 --dir "$L/db-T" --queries "$Q" --only q10 --max-load 100000 --load-wait-s 0 --targets amphora --runs 5 --timeout-s 120 --count-timeout-s 120 --pin-weights "$P" "$@"; }
for arm in ${ARMS:-B A}; do
  case $arm in A) export AMPHORA_STAR_FILTERS=off;; B) export AMPHORA_STAR_FILTERS=bycost;; esac
  echo "=== pc $arm $(date -u +%H:%M:%SZ) $(state)"
  AMPHORA_PC_TRACE=1 b --out "$L/traced/pc-$arm.jsonl" 2> "$L/traced/pc-$arm.log" || { echo "rc=$?"; tail -3 "$L/traced/pc-$arm.log"; }
  AMPHORA_PLAN_TRACE=1 AMPHORA_SEEK_TRACE=1 b --out "$L/traced/seek-$arm.jsonl" 2> "$L/traced/seek-$arm.log" || { echo "rc=$?"; tail -3 "$L/traced/seek-$arm.log"; }
  python3 -I "$L/pc.py" "$L/traced/pc-$arm.log" "$L/traced/pc-$arm.jsonl" 0.3
  grep -E 'T18: a cursor.s seeks this run|T19: the star filters' "$L/traced/seek-$arm.log" | sed 's/^[^ ]* *DEBUG //' | cut -c1-210 | tail -4
done
echo "=== end $(date -u +%H:%M:%SZ) $(state)"
