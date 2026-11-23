set -uo pipefail
BIN=${BIN:-$HOME/.cache/amphora-perf-bin/bench-m2/29eb13ba10f2e46176bfd4ebdd07c2cd89f1fa8e/amphora-bench}
SRC=$HOME/.cache/amphora-perf/bench-m2-remote; TPCH=$HOME/.cache/planner-bench/tpch-sf1; W=$HOME/.cache/amphora-g6; DB=$W/db/tpch-sf1
mkdir -p $W/ab; [ -d $DB/amphora-vectorized ] || cp -a --reflink=always $DB/amphora $DB/amphora-vectorized
cd $SRC || exit 3
snap() { echo "$(date -u +%FT%TZ) $1 load=$(cut -d' ' -f1-3 /proc/loadavg) cpu[$(head -1 /proc/pressure/cpu)] io[$(head -1 /proc/pressure/io)]"; }
for i in $(seq 1 ${ROUNDS:-2}); do for order in amphora,amphora-vectorized amphora-vectorized,amphora; do
  snap "round $i $order"
  $BIN --planner --bench tpch --sf 1 --data $TPCH --queries bench/planner/tpch/queries --dir $DB --targets $order --only ${QS:-q01,q06} --runs ${RUNS:-5} --timeout-s 60 --count-timeout-s 60 --load-wait-s 0 --out $W/ab/vec${TAG:-}-$i-${order%%,*}.jsonl > $W/ab/vec${TAG:-}-$i-${order%%,*}.log 2>&1 || echo "rc=$?"
done; done
snap end
python3 -I $W/summ.py $W/ab/vec${TAG:-}-*.jsonl
