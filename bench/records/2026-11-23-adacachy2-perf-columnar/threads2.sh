set -uo pipefail
BIN=${BIN:-$HOME/.cache/amphora-perf-bin/bench-m2/29eb13ba10f2e46176bfd4ebdd07c2cd89f1fa8e/amphora-bench}
TPCH=$HOME/.cache/planner-bench/tpch-sf1; W=$HOME/.cache/amphora-g6; DB=$W/db2/tpch-sf1; SRC=$HOME/.cache/amphora-perf/bench-m2-remote
rm -rf $W/qi-root && mkdir -p $W/qi-root && cp -r $SRC/bench/planner/tpch/queries $W/qi-root/queries && cp $SRC/bench/planner/tpch/*.sql $W/qi-root/ && cp $W/qx/q06i.sql $W/qi-root/queries/q06.sql && cp $W/qx/q01i.sql $W/qi-root/queries/q01.sql && mkdir -p $W/db2/tpch-sf1 && { [ -d $W/db2/tpch-sf1/amphora ] || cp -a --reflink=always $HOME/.cache/amphora-bench-m2/db/tpch-sf1/amphora $W/db2/tpch-sf1/; }
mkdir -p $W/thr; cd $W || exit 3
probe() { local tag=$1 qd=$2 q=$3 c=$4
  $BIN --planner --bench tpch --sf 1 --data $TPCH --queries $qd --dir $DB --targets amphora --cores $c --only $q --runs 3 --timeout-s 30 --count-timeout-s 30 --load-wait-s 0 --out $W/thr/$tag-$q-c$c.jsonl > $W/thr/$tag-$q-c$c.log 2>&1 &
  P=$!; : > $W/thr/$tag-$q-c$c.tasks
  while kill -0 $P 2>/dev/null; do
    for t in /proc/$P/task/*; do s=$(cat $t/stat 2>/dev/null) || continue; v=$(grep -h ctxt_switches $t/status 2>/dev/null | awk '{printf "%s ", $2}'); echo "$(date +%s.%N) ${t##*/} $v| $s"; done >> $W/thr/$tag-$q-c$c.tasks
    sleep 0.02
  done
  wait $P; echo "== $tag $q c$c rc=$? $(grep cores $W/thr/$tag-$q-c$c.log | tail -1) load=$(cut -d' ' -f1 /proc/loadavg)"
  python3 -I $W/tasks.py $W/thr/$tag-$q-c$c.tasks | head -11
}
for c in 1 8; do probe int $W/qi-root/queries q06 $c; probe int $W/qi-root/queries q01 $c; probe real $SRC/bench/planner/tpch/queries q06 $c; done
python3 -I $W/summ.py $W/thr/int-*.jsonl $W/thr/real-*.jsonl
