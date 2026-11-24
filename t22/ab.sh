#!/usr/bin/env bash
# Lane T22 step 1: q10 alone at SF1, A (main without T19's a34ec88b, b0f09f9a) against B (main a64549cc), T17's S weights
# pinned, alternated (A B, then B A, ...), ROUNDS rounds, each arm a process: the counted run, then 5 timed runs. Both
# binaries are T21's. Their calibration files are T21's (one body, sha256 b7661c2007aeb06d, under both keys). One
# reflink copy of T11's db-sf1 per arm. The load and CPU pressure are printed at each process's start and end, and each
# run's load is in its record. Then: the answers (every process), a traced process per arm, and the summary.
set -uo pipefail
ROUNDS=${ROUNDS:-20} STAGES=${STAGES:-db,ab,trace,summary}
A=b0f09f9a033929db1618c83f59f426c17573f5de B=a64549cce7f0406929ca64ff32a61255c5821d87
BIN=~/.cache/amphora-perf-bin/tpch L=~/.cache/tpch-t22 T21=~/.cache/tpch-t21 T11=~/.cache/tpch-t11 Q=${Q:-~/.cache/tpch-t22/queries}
XDG=$L/xdg P=$L/S.json
state() { echo "load $(cut -d' ' -f1-3 /proc/loadavg) cpu_psi $(head -1 /proc/pressure/cpu | cut -d' ' -f2) io_psi $(head -1 /proc/pressure/io | cut -d' ' -f2)"; }
has() { case ",$STAGES," in *",$1,"*) return 0;; esac; return 1; }
sha_of() { case $1 in A) echo "$A";; B) echo "$B";; esac; }
mkdir -p "$L/out" "$L/ans" "$L/traced" || exit 1
if has db; then
  mkdir -p "$XDG" && cp -a "$T21/xdg/." "$XDG/" || exit 1
  cp "$T21/S.json" "$P" || exit 1
  for a in A B; do [ -d "$L/db-$a" ] || cp -a --reflink=always "$T11/db-sf1" "$L/db-$a" || exit 1; done
  mkdir -p "$Q" && cp "$L/q10.sql" "$Q/q10.sql" || exit 1
  for a in A B; do sha256sum "$BIN/amphora-bench-$(sha_of $a)" | cut -c1-16; done
fi
b() { local arm=$1; shift; XDG_CACHE_HOME=$XDG nice -n 10 "$BIN/amphora-bench-$(sha_of "$arm")" --planner --bench tpch --sf 1 --data ~/.cache/planner-bench/tpch-sf1 --dir "$L/db-$arm" --queries "$Q" --only q10 --max-load 100000 --load-wait-s 0 --targets amphora "$@"; }
TIMEFORMAT='cpu_s %U %S wall_s %R'
echo "start $(date -u +%H:%M:%SZ) $(state) nproc $(nproc)"
if has ab; then
  for i in $(seq 1 "$ROUNDS"); do
    case $((i % 2)) in 1) order="A B";; *) order="B A";; esac
    for arm in $order; do
      echo "=== $arm r$i $(date -u +%H:%M:%SZ) $(state)"
      { time b "$arm" --runs 5 --timeout-s 120 --count-timeout-s 120 --pin-weights "$P" --answers "$L/ans/$arm-r$i" --out "$L/out/$arm-r$i.jsonl" 2> "$L/out/$arm-r$i.log"; } 2>&1
      echo "rc=$? $(state)"
    done
  done
fi
if has trace; then
  for arm in A B; do
    echo "=== traced $arm $(date -u +%H:%M:%SZ) $(state)"
    AMPHORA_PLAN_TRACE=1 AMPHORA_SEEK_TRACE=1 b "$arm" --runs 5 --timeout-s 120 --count-timeout-s 120 --pin-weights "$P" --out "$L/traced/$arm-q10.jsonl" 2> "$L/traced/$arm-q10.log" || echo "traced $arm rc=$?"
  done
fi
if has summary; then python3 -I "$L/summ22.py" "$L" > "$L/summary.txt" 2>&1; echo "=== summary"; head -c 12000 "$L/summary.txt"; fi
echo "=== end $(date -u +%H:%M:%SZ) $(state)"
