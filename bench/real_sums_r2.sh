#!/usr/bin/env bash
# Lane R2 (reals): what bench/real_sums_ab.sh's run leaves out, on that run's binary and database.
# Run it after that run ends, never beside it: both open the same database.
#
# usage: bench/real_sums_r2.sh <scale factor> <out dir> <binary's sha> <phase>...
#   ref          bench/real_sums_ref.py for q09 (KBN's loss) and TPC-H q01, q06 into ref-r2.json
#   answers-kbn  each arm at 1, 4, 8 and 16 workers, and SQLite, on bench/planner/real_sums q09
#   answers-b    each arm at 1, 4 and 8 workers on the other REAL-summing TPC-H queries (the ab
#                script's answers-b runs 16)
#   limit        R1's side finding: `SELECT 1 FROM bench_loaded LIMIT 1` at 1, 4, 8, 16 and 32 workers
set -euo pipefail
SF=${1:?scale factor}
OUT=${2:?out dir}
SHA=${3:?binary sha}
shift 3
BIN=$HOME/.cache/amphora-perf-bin/reals/$SHA/amphora-bench
DB=$HOME/.cache/amphora-perf/reals-db/sf$SF
DATA=$HOME/.cache/amphora-perf/reals-data/tpch-sf$SF
QB=q03,q05,q07,q08,q09,q10,q11,q14,q15,q17,q19,q22
COMMON="--planner --bench tpch --sf $SF --data $DATA --dir $DB --max-load 1000000 --load-wait-s 0"
note() { echo "$(date -u +%Y-%m-%dT%H:%M:%SZ) r2 $* load $(sysctl -n vm.loadavg 2>/dev/null || cut -d' ' -f1-3 /proc/loadavg)" | tee -a "$OUT/phases.log"; }
bench() { # <arm> <cores or -> <args>...
  local arm=$1 cores=$2
  shift 2
  if [ "$cores" = - ]; then
    AMPHORA_REAL_SUMS=$arm "$BIN" $COMMON "$@"
  else
    AMPHORA_REAL_SUMS=$arm AMPHORA_BENCH_CORES=$cores "$BIN" $COMMON "$@"
  fi
}
[ -x "$BIN" ] || { echo "no binary $BIN"; exit 2; }
note "start $SHA phases $*"
for phase in "$@"; do
  note "phase $phase"
  case $phase in
    ref) python3 -I bench/real_sums_ref.py "$DATA" q09,tpch-q01,tpch-q06 >"$OUT/ref-r2.json" 2>"$OUT/ref-r2.err" ;;
    answers-kbn)
      for arm in serial exact roworder; do
        for c in 1 4 8 16; do
          note "answers-kbn $arm $c"
          bench "$arm" "$c" --queries bench/planner/real_sums/queries --targets amphora --only q09 --runs 1 --timeout-s 600 --answers "$OUT/answers-hard/$arm-c$c" --out "$OUT/answers-kbn-$arm-c$c.jsonl" 2>>"$OUT/answers-r2.err"
        done
      done
      bench serial - --queries bench/planner/real_sums/queries --targets sqlite --only q09 --runs 1 --timeout-s 600 --answers "$OUT/answers-hard/sqlite" --out "$OUT/answers-kbn-sqlite.jsonl" 2>>"$OUT/answers-r2.err"
      ;;
    answers-b)
      for arm in serial exact roworder; do
        for c in 1 4 8; do
          note "answers-b $arm $c"
          bench "$arm" "$c" --queries bench/planner/tpch/queries --targets amphora --only $QB --runs 1 --timeout-s 600 --answers "$OUT/answers-b/$arm-c$c" --out "$OUT/answers-b-$arm-c$c.jsonl" 2>>"$OUT/answers-r2.err"
        done
      done
      ;;
    limit)
      mkdir -p "$OUT/limit-queries"
      echo "SELECT 1 FROM bench_loaded LIMIT 1" >"$OUT/limit-queries/q01.sql"
      for c in 1 4 8 16 32; do
        note "limit $c"
        bench serial "$c" --queries "$OUT/limit-queries" --targets amphora --only q01 --runs 3 --timeout-s 60 --answers "$OUT/limit/c$c" --out "$OUT/limit-c$c.jsonl" 2>>"$OUT/limit.err" || echo "limit at $c cores: exit $?" | tee -a "$OUT/limit.err"
        echo "limit at $c cores: $(cat "$OUT/limit/c$c/tpch-sf$SF/q01/amphora.tsv" 2>/dev/null | head -3 | tr '\n' ' ')" | tee -a "$OUT/limit.err"
      done
      ;;
    *) echo "unknown phase $phase"; exit 2 ;;
  esac
done
note "end"
