#!/usr/bin/env bash
# Lane R1 (reals): the SUM/AVG-over-REAL A/B on TPC-H through the planner bench. Arms are
# RealSums Serial (today), Exact (arm b) and RowOrder (arm a), chosen per process by
# AMPHORA_REAL_SUMS from ONE binary, so every arm plans from one calibration (RULES 5e: one
# executable, one key). DuckDB (duckdb-mt, `SET threads = n`) runs in the same rotation.
#
# usage: bench/real_sums_ab.sh <scale factor> <out dir> <phase>...
#   build    release amphora-bench with the root's shipping profile, copied to ~/.cache/amphora-perf-bin/reals/<sha>
#   data     dbgen at the scale factor (an existing dbgen output of the same revision is reused)
#   load     load amphora, sqlite and duckdb-mt once (--only -)
#   answers  each arm at 1, 4, 8 and 16 workers (AMPHORA_BENCH_CORES) with --answers, and SQLite's
#            own, on bench/planner/real_sums/queries (hard cases) and TPC-H q01, q06
#   timed-a  ROUNDS rounds (default 5), the arms and DuckDB alternated, q01 and q06 on the ladder
#   timed-b  ROUNDS_B rounds (default 1), the other REAL-summing TPC-H queries on the ladder
#   answers-b  each arm at 16 workers and SQLite on those queries
set -euo pipefail
SF=${1:?scale factor}
OUT=${2:?out dir}
shift 2
REV=32f1c1b92d1664dba542e927d23d86ffa57aa253
SHA=$(git rev-parse --short=8 HEAD)
export PATH=$HOME/.cargo/bin:$PATH
export CARGO_HOME=$HOME/.cache/cargo-home-perf-reals CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/reals
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-16} CARGO_INCREMENTAL=0
export CARGO_PROFILE_RELEASE_LTO=thin CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS=true
BIN=$HOME/.cache/amphora-perf-bin/reals/$SHA/amphora-bench
DB=$HOME/.cache/amphora-perf/reals-db/sf$SF
CORES=1,4,8,16
QA=q01,q06
QB=q03,q05,q07,q08,q09,q10,q11,q14,q15,q17,q19,q22
HARD=q01,q02,q03,q04,q05,q06,q07,q08
mkdir -p "$OUT"

note() { echo "$(date -u +%Y-%m-%dT%H:%M:%SZ) $* load $(sysctl -n vm.loadavg 2>/dev/null || cut -d' ' -f1-3 /proc/loadavg)" | tee -a "$OUT/phases.log"; }

data_dir() {
  local d
  for d in $(find "$HOME/.cache" -maxdepth 5 -type f -name REVISION -path "*tpch-sf$SF/REVISION" 2>/dev/null); do
    d=$(dirname "$d")
    if [ "$(cat "$d/REVISION")" = "$REV" ] && [ -s "$d/lineitem.tbl" ]; then echo "$d"; return; fi
  done
  echo "$HOME/.cache/amphora-perf/reals-data/tpch-sf$SF"
}
DATA=$(data_dir)
COMMON="--planner --bench tpch --sf $SF --data $DATA --dir $DB --max-load 1000000 --load-wait-s 0"

bench() { # <arm> <cores or -> <args>...
  local arm=$1 cores=$2
  shift 2
  if [ "$cores" = - ]; then
    AMPHORA_REAL_SUMS=$arm "$BIN" $COMMON "$@"
  else
    AMPHORA_REAL_SUMS=$arm AMPHORA_BENCH_CORES=$cores "$BIN" $COMMON "$@"
  fi
}

rounds() { # <name> <queries> <rounds> <runs>
  local name=$1 q=$2 n=$3 runs=$4 r i arm
  local arms=(serial exact roworder duckdb)
  for r in $(seq 1 "$n"); do
    for i in 0 1 2 3; do
      arm=${arms[$(((i + r - 1) % 4))]}
      note "$name round $r $arm"
      if [ "$arm" = duckdb ]; then
        bench serial - --queries bench/planner/tpch/queries --targets duckdb-mt --only "$q" --cores $CORES --runs "$runs" --timeout-s 300 --out "$OUT/$name-$arm-r$r.jsonl" 2>>"$OUT/$name.err"
      else
        bench "$arm" - --queries bench/planner/tpch/queries --targets amphora --only "$q" --cores $CORES --runs "$runs" --timeout-s 300 --out "$OUT/$name-$arm-r$r.jsonl" 2>>"$OUT/$name.err"
      fi
    done
  done
}

[ "$(df -g ~ 2>/dev/null | tail -1 | awk '{print $4}')" -ge 40 ] 2>/dev/null || [ "$(uname)" != Darwin ] || { echo "disk low"; exit 2; }
note "start $SHA on $(hostname) ($(sysctl -n hw.ncpu 2>/dev/null || nproc) cores) data $DATA phases $*"

for phase in "$@"; do
  note "phase $phase"
  case $phase in
    build)
      mkdir -p "$CARGO_HOME"
      for d in registry git bin; do [ -e "$CARGO_HOME/$d" ] || ln -s "$HOME/.cargo/$d" "$CARGO_HOME/$d"; done
      cp Cargo.lock bench/Cargo.lock
      nice -n 10 cargo build --release --manifest-path bench/Cargo.toml --offline || nice -n 10 cargo build --release --manifest-path bench/Cargo.toml
      mkdir -p "$(dirname "$BIN")"
      cp "$CARGO_TARGET_DIR/release/amphora-bench" "$BIN"
      cp bench/Cargo.lock "$OUT/Cargo.lock-$SHA"
      ;;
    data)
      if [ ! -f "$DATA/REVISION" ]; then
        src=$HOME/.cache/amphora-perf/reals-dbgen
        if [ ! -d "$src/.git" ] && [ -d "$HOME/.cache/planner-bench/tpch-dbgen/.git" ]; then cp -R -c "$HOME/.cache/planner-bench/tpch-dbgen" "$src" 2>/dev/null || cp -R "$HOME/.cache/planner-bench/tpch-dbgen" "$src"; fi
        TPCH_DBGEN_SRC=$src bench/planner/tpch/dbgen.sh "$SF" "$DATA"
      fi
      ;;
    load)
      bench serial - --queries bench/planner/tpch/queries --targets amphora,sqlite,duckdb-mt --only - --runs 1 --out "$OUT/load.jsonl" 2>>"$OUT/load.err"
      ;;
    answers)
      for arm in serial exact roworder; do
        for c in 1 4 8 16; do
          note "answers $arm $c"
          bench "$arm" "$c" --queries bench/planner/real_sums/queries --targets amphora --only $HARD --runs 1 --timeout-s 600 --answers "$OUT/answers-hard/$arm-c$c" --out "$OUT/answers-hard-$arm-c$c.jsonl" 2>>"$OUT/answers.err"
          bench "$arm" "$c" --queries bench/planner/tpch/queries --targets amphora --only $QA --runs 1 --timeout-s 600 --answers "$OUT/answers-tpch/$arm-c$c" --out "$OUT/answers-tpch-$arm-c$c.jsonl" 2>>"$OUT/answers.err"
        done
      done
      bench serial - --queries bench/planner/real_sums/queries --targets sqlite --only $HARD --runs 1 --timeout-s 600 --answers "$OUT/answers-hard/sqlite" --out "$OUT/answers-hard-sqlite.jsonl" 2>>"$OUT/answers.err"
      bench serial - --queries bench/planner/tpch/queries --targets sqlite --only $QA --runs 1 --timeout-s 600 --answers "$OUT/answers-tpch/sqlite" --out "$OUT/answers-tpch-sqlite.jsonl" 2>>"$OUT/answers.err"
      ;;
    timed-a) rounds timed-a "$QA" "${ROUNDS:-5}" "${RUNS:-5}" ;;
    timed-b) rounds timed-b "$QB" "${ROUNDS_B:-1}" "${RUNS_B:-3}" ;;
    answers-b)
      for arm in serial exact roworder; do
        note "answers-b $arm 16"
        bench "$arm" 16 --queries bench/planner/tpch/queries --targets amphora --only $QB --runs 1 --timeout-s 600 --answers "$OUT/answers-b/$arm-c16" --out "$OUT/answers-b-$arm-c16.jsonl" 2>>"$OUT/answers.err"
      done
      bench serial - --queries bench/planner/tpch/queries --targets sqlite --only $QB --runs 1 --timeout-s 600 --answers "$OUT/answers-b/sqlite" --out "$OUT/answers-b-sqlite.jsonl" 2>>"$OUT/answers.err"
      ;;
    *) echo "unknown phase $phase"; exit 2 ;;
  esac
done
note "end"
