set -uo pipefail
BIN=${BIN:-$HOME/.cache/amphora-perf-bin/bench-m2/29eb13ba10f2e46176bfd4ebdd07c2cd89f1fa8e/amphora-bench}
SRC=$HOME/.cache/amphora-perf/bench-m2-remote
TPCH=$HOME/.cache/planner-bench/tpch-sf1
W=$HOME/.cache/amphora-g6
cd $SRC || exit 3
snap() { echo "$(date -u +%FT%TZ) $1 load=$(cut -d' ' -f1-3 /proc/loadavg) cpu[$(head -1 /proc/pressure/cpu)]"; }
ARGS() { echo --planner --bench tpch --sf 1 --data $TPCH --queries bench/planner/tpch/queries --dir $W/db/tpch-sf1 --targets ${TGT:-amphora} --cores $2 --only $1 --runs ${RUNS:-2} --timeout-s 30 --count-timeout-s 30 --load-wait-s 0; }
for q in $QS; do for c in $CS; do
  snap "$q c$c pmp start"
  PMP_OUT=$W/prof/$q-c$c$TAG.pmp gdb -q -batch -x $W/pmp.py --args $BIN $(ARGS $q $c) --out $W/prof/$q-c$c$TAG-pmp.jsonl > $W/prof/$q-c$c$TAG-gdb.log 2>&1 &
  G=$!; INF=""
  for i in $(seq 1 100); do INF=$(pgrep -P $G | head -1); [ -n "$INF" ] && break; sleep 0.1; done
  while [ -n "$INF" ] && kill -0 $INF 2>/dev/null; do kill -INT $INF 2>/dev/null; sleep ${PERIOD:-0.05}; done
  wait $G; echo "$q c$c gdb rc=$? samples=$(grep -c -- '^--' $W/prof/$q-c$c$TAG.pmp)"
  snap "$q c$c procs start"
  $BIN $(ARGS $q $c) --out $W/prof/$q-c$c$TAG-procs.jsonl > $W/prof/$q-c$c$TAG-procs.log 2>&1 &
  P=$!; : > $W/prof/$q-c$c$TAG.tasks
  while kill -0 $P 2>/dev/null; do
    for t in /proc/$P/task/*; do s=$(cat $t/stat 2>/dev/null) || continue; v=$(grep -h ctxt_switches $t/status 2>/dev/null | awk '{printf "%s ", $2}'); echo "$(date +%s.%N) ${t##*/} $v| $s"; done >> $W/prof/$q-c$c$TAG.tasks
    sleep 0.02
  done
  wait $P; echo "$q c$c procs rc=$?"; grep -E "cores" $W/prof/$q-c$c$TAG-procs.log | tail -3
done; done
snap end
