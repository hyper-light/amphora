#!/usr/bin/env bash
# Lane S6: one run of every competitor family that runs today, at main cfa0107c, built with the
# workspace's release profile (lto thin, codegen-units 1, overflow-checks, line tables).
# Lane A: TPC-H SF1, all 22 queries, amphora vs sqlite vs duckdb (one thread each), answers held to each other.
# Lane B: replication ladder (writers 1/32/128), then the SQL suite in process (amphora, turso, sqlite),
#         then the suite on PostgreSQL in its container.
# The two lanes run side by side; phases.log has the load, CPU and IO pressure every 15 s.
set -uo pipefail
SHA=cfa0107c5e45833823fc4f52e9bd238159ae9293
L=~/.cache/amphora-perf/bench-s6-run
O=$L/out
mkdir -p "$O"
exec 8>"$L/build.lock"; flock 8; exec 8>&-
[ "$(cat "$L/build-ok" 2>/dev/null)" = "$SHA" ] || { echo "no build at $SHA"; exit 1; }
. ~/.cache/amphora-perf/bench-s2/env.sh
export CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=8 CARGO_PROFILE_RELEASE_DEBUG=line-tables-only
SRC=~/.cache/amphora-perf/bench-s
cd "$SRC" || exit 1
[ "$(git rev-parse HEAD)" = "$SHA" ] || { echo "clone not at $SHA"; exit 1; }
BIN=$CARGO_TARGET_DIR/release/amphora-bench
cmp -s "$BIN" ~/.cache/amphora-perf-bin/bench-s6/amphora-bench-$SHA || { echo "binary is not the build's"; exit 1; }
PB=~/.cache/planner-bench
S() { echo "$(date -u +%FT%TZ) $1 load=$(cut -d' ' -f1-3 /proc/loadavg) cpu[$(head -1 /proc/pressure/cpu)] io_full[$(sed -n 2p /proc/pressure/io)]" >>"$O/phases.log"; }
(while :; do S sample; sleep 15; done) &
MON=$!
trap 'kill $MON 2>/dev/null; bash bench/servers/postgres.sh stop >/dev/null 2>&1' EXIT
END=$(date -d "${END_AT:-2026-11-23T20:06:00Z}" +%s)
export AMPHORA_BENCH_DATA=$L/data
mkdir -p "$AMPHORA_BENCH_DATA" && chattr +C "$AMPHORA_BENCH_DATA" 2>/dev/null
left() { local s=$((END - $(date +%s))); [ $s -gt 5 ] && echo $s || echo 5; }

lane_a() {
  local T=(--planner --bench tpch --sf 1 --data "$PB/tpch-sf1" --queries "$SRC/bench/planner/tpch/queries" --dir "$L/db/tpch-sf1"
    --targets amphora,sqlite,duckdb --max-load 100000 --load-wait-s 0)
  S tpch-load-start
  XDG_CACHE_HOME=$L/xdg timeout -k 10 "$(left)" nice -n 10 "$BIN" "${T[@]}" --only - --out "$O/tpch-load.jsonl" >"$O/tpch-load.log" 2>&1
  S "tpch-load-end rc=$?"
  S tpch-start
  XDG_CACHE_HOME=$L/xdg timeout -k 10 "$(left)" nice -n 10 "$BIN" "${T[@]}" --runs 3 --timeout-s 60 --count-timeout-s 60 --resume \
    --answers "$O/ans-tpch" --out "$O/tpch.jsonl" >"$O/tpch.log" 2>&1
  S "tpch-end rc=$?"
  python3 -I bench/planner/answers.py "$O/ans-tpch" >"$O/tpch-answers.txt" 2>&1
  echo "answers.py rc=$?" >>"$O/tpch-answers.txt"
}

lane_b() {
  S ladder-start
  SYSTEMS="amphora-procs-persisted amphora-procs-replicated rqlite dqlite libsql litefs" LADDER="1 32 128" \
    timeout -k 10 "$(left)" nice -n 10 bash bench/repl_ladder.sh "$O/repl" --batch 1 --text-bytes 600 --deadline-s 10 \
    --relay-records 128 --spanning 2 --warmup-s 1 --seconds 5 >"$O/repl.tsv" 2>"$O/repl.err"
  S "ladder-end rc=$?"
  local only='point_pk$,point_secondary$,point_text$,json_field$,blob_read$,insert_one$,insert_batch_100$,update_plain$,update_indexed$,upsert$,delete_insert$,txn_read_write$,table_count$,table_group_by$,filter_no_index$,aggregates_range$,join_three$'
  local args=(--only "$only" --items 200000 --clients 1,32,128 --seconds 1 --warmup 0.5)
  S suite-inproc-start
  SUITE_OUT=$O/suite-inproc.jsonl timeout -k 10 "$(left)" nice -n 10 bash bench/suite.sh amphora,turso,sqlite "${args[@]}" 2>"$O/suite-inproc.err"
  S "suite-inproc-end rc=$?"
  S suite-pg-start
  SUITE_OUT=$O/suite-pg.jsonl timeout -k 10 "$(left)" nice -n 10 bash bench/suite.sh postgres "${args[@]}" 2>"$O/suite-pg.err"
  S "suite-pg-end rc=$?"
}

lane_a &
PA=$!
lane_b &
PB_=$!
wait $PA
wait $PB_
S done
ls -la "$O"
