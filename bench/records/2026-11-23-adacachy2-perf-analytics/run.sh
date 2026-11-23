#!/usr/bin/env bash
# Lane M2: TPC-H SF1 + JOB, Amphora vs SQLite vs DuckDB on adacachy2, shipping profile.
set -uo pipefail
SHA=${1:?sha}
TIMEOUT=${TIMEOUT:-60}
SRC=$HOME/.cache/amphora-perf/bench-m2-remote
OUT=$HOME/.cache/amphora-bench-m2/out/$SHA
BIN=$HOME/.cache/amphora-perf-bin/bench-m2/$SHA/amphora-bench
PB=$HOME/.cache/planner-bench
mkdir -p "$OUT" "$(dirname "$BIN")"
snap() { echo "$(date -u +%FT%TZ) $1 load=$(cut -d' ' -f1-3 /proc/loadavg) cpu[$(head -1 /proc/pressure/cpu)] io[$(head -1 /proc/pressure/io)] iofull[$(sed -n 2p /proc/pressure/io)] mem_avail_kib=$(awk '/MemAvailable/{print $2}' /proc/meminfo)" >>"$OUT/phases.log"; }
( while :; do snap sample; sleep 30; done ) & SAMPLER=$!
trap 'kill $SAMPLER 2>/dev/null' EXIT
if [[ ! -d $SRC/.git ]]; then git clone -q https://github.com/hyper-light/amphora.git "$SRC" || git clone -q git@github.com:hyper-light/amphora.git "$SRC"; fi
cd "$SRC" && git fetch -q origin perf/bench-m2 main && git checkout -q --detach "$SHA" || exit 3
if [[ ! -x $BIN ]]; then
  snap build-start
  export CARGO_HOME=$HOME/.cache/cargo-home-perf-bench-s CARGO_NET_OFFLINE=true CARGO_INCREMENTAL=0 \
    CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/bench-s CARGO_BUILD_JOBS=16 \
    CARGO_PROFILE_RELEASE_LTO=thin CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS=true
  env | grep -E '^CARGO_PROFILE' >"$OUT/profile-$SHA.txt"
  nice -n 10 cargo build --release --manifest-path bench/Cargo.toml --bin amphora-bench >"$OUT/build.log" 2>&1; rc=$?
  snap "build-end rc=$rc"; [[ $rc == 0 ]] || { tail -30 "$OUT/build.log"; exit 4; }
  cp "$CARGO_TARGET_DIR/release/amphora-bench" "$BIN.tmp" && mv "$BIN.tmp" "$BIN"
  cp bench/Cargo.lock "$OUT/Cargo.lock-$SHA" 2>/dev/null || cp Cargo.lock "$OUT/Cargo.lock-$SHA"
fi
TPCH=$PB/tpch-sf1
if [[ $(stat -c %s "$TPCH/lineitem.tbl" 2>/dev/null || echo 0) -lt 700000000 ]]; then
  TPCH=$HOME/.cache/amphora-bench-m2/tpch-sf1; [[ -s $TPCH/lineitem.tbl ]] || bash bench/planner/tpch/dbgen.sh 1 "$TPCH" || exit 5
fi
ls -la "$TPCH" >"$OUT/tpch-data.txt"
DB=$HOME/.cache/amphora-bench-m2/db
COMMON=(--runs 5 --timeout-s "$TIMEOUT" --count-timeout-s "$TIMEOUT" --load-wait-s 0 --resume)
phase() { local name=$1; shift; snap "$name-start"; "$BIN" --planner "$@" >"$OUT/$name.log" 2>&1; local rc=$?; snap "$name-end rc=$rc"; }
ENG=amphora,amphora-adaptive,sqlite,duckdb,duckdb-mt
phase tpch --bench tpch --sf 1 --data "$TPCH" --queries bench/planner/tpch/queries --dir "$DB/tpch-sf1" --targets $ENG "${COMMON[@]}" --answers "$OUT/answers" --out "$OUT/tpch.jsonl"
phase tpch-cores --bench tpch --sf 1 --data "$TPCH" --queries bench/planner/tpch/queries --dir "$DB/tpch-sf1" --targets amphora,duckdb-mt --cores 1,2,4,8,16 "${COMMON[@]}" --out "$OUT/tpch-cores.jsonl"
phase job --bench job --data "$PB/imdb" --queries "$PB/join-order-benchmark" --dir "$DB/job" --targets $ENG "${COMMON[@]}" --answers "$OUT/answers" --out "$OUT/job.jsonl"
phase job-cores --bench job --data "$PB/imdb" --queries "$PB/join-order-benchmark" --dir "$DB/job" --targets amphora,duckdb-mt --cores 1,2,4,8,16 "${COMMON[@]}" --out "$OUT/job-cores.jsonl"
echo done
