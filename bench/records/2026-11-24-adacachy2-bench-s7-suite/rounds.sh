#!/usr/bin/env bash
# Lane S7 (bench): the in-process SQL suite, Amphora against Turso and SQLite, at main $SHA, on adacachy2.
# The op set, items, clients, seconds and warmup are the ones S4 and S6 ran (S6's 17 statements; S4 ran all 52).
# $ROUNDS rounds, order of targets reversed every round. One smoke pass first (clients 1 and 128, short window),
# then the answers of every target held to each other, then the timed rounds. Sampler: load and pressure every 5 s.
# usage: rounds.sh smoke|rounds|all
set -uo pipefail
SHA=0101aa386850d1e8e641c6254238567adf9d8aa7
L=~/.cache/amphora-perf/bench-s7-run
O=$L/out; mkdir -p "$O"
ROUNDS=${ROUNDS:-5}
MODE=${1:-all}
exec 9>~/.cache/amphora-perf/bench-s2/timed.lock
flock -w 300 9 || { echo "timed.lock held by another run"; exit 1; }
[ "$(cat "$L/build-ok" 2>/dev/null)" = "$SHA" ] || { echo "no build at $SHA"; exit 1; }
. ~/.cache/amphora-perf/bench-s2/env.sh
export CARGO_INCREMENTAL=0 CARGO_PROFILE_RELEASE_DEBUG=line-tables-only
export CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/bench-s
SRC=~/.cache/amphora-perf/bench-s
cd "$SRC" || exit 1
[ "$(git rev-parse HEAD)" = "$SHA" ] || { echo "clone not at $SHA"; exit 1; }
[ -z "$(git status --porcelain)" ] || { echo "clone is dirty"; git status --porcelain; exit 1; }
BIN=$CARGO_TARGET_DIR/release/amphora-bench
SAVED=~/.cache/amphora-perf-bin/bench-s7/amphora-bench-$SHA
cmp -s "$BIN" "$SAVED" || { echo "binary is not the build's"; exit 1; }
P() { echo "$1 $(date -u +%FT%TZ) load $(cut -d' ' -f1-3 /proc/loadavg) | cpu $(sed -n 1p /proc/pressure/cpu) | io $(sed -n 1p /proc/pressure/io) / $(sed -n 2p /proc/pressure/io)"; }
S() { echo "$(date -u +%FT%TZ) $1 load=$(cut -d' ' -f1-3 /proc/loadavg) cpu[$(sed -n 1p /proc/pressure/cpu)] io_some[$(sed -n 1p /proc/pressure/io)] io_full[$(sed -n 2p /proc/pressure/io)]"; }
procs() { echo "== $1 $(date -u +%FT%TZ)"; ps -eo pid,user,ni,pcpu,etimes,comm,args --sort=-pcpu | head -9 | cut -c1-230; }
ONLY='point_pk$,point_secondary$,point_text$,json_field$,blob_read$,insert_one$,insert_batch_100$,update_plain$,update_indexed$,upsert$,delete_insert$,txn_read_write$,table_count$,table_group_by$,filter_no_index$,aggregates_range$,join_three$'
ARGS=(--only "$ONLY" --items 200000 --clients 1,32,128 --seconds 1 --warmup 0.5)
A=amphora,turso,sqlite B=sqlite,turso,amphora
(while :; do S sample >>"$O/phases.log"; sleep 5; done) &
MON=$!
trap 'kill $MON 2>/dev/null' EXIT

if [ "$MODE" = smoke ] || [ "$MODE" = all ]; then
  echo "$SHA" >"$O/sha.txt"
  P "smoke-start" >>"$O/rounds.txt"; procs "smoke" >>"$O/procs.txt"
  rm -f "$O/smoke.jsonl"
  SUITE_OUT=$O/smoke.jsonl nice -n 10 bash bench/suite.sh $A --only "$ONLY" --items 200000 --clients 1,128 --seconds 0.3 --warmup 0.1 2>"$O/smoke.err"
  P "smoke-end" >>"$O/rounds.txt"
  python3 -I bench/suite_answers.py "$O/smoke.jsonl" >"$O/smoke-answers.txt" 2>&1; echo "suite_answers rc=$?" >>"$O/smoke-answers.txt"
  tail -15 "$O/smoke-answers.txt"
  echo "calibration files after the smoke pass:"; ls -la "${XDG_CACHE_HOME:-$HOME/.cache}/amphora/calibration" 2>/dev/null | tail -5
  python3 -I "$L/scripts/smoke_summary.py" "$O/smoke.jsonl"
fi

if [ "$MODE" = rounds ] || [ "$MODE" = all ]; then
  for i in $(seq "${START:-1}" "$ROUNDS"); do
    s=$A; [ $((i % 2)) = 0 ] && s=$B
    P "round $i start ($s)" | tee -a "$O/rounds.txt"; procs "round $i" >>"$O/procs.txt"
    SUITE_OUT=$O/round$i.jsonl nice -n 10 bash bench/suite.sh $s "${ARGS[@]}" 2>"$O/round$i.err"
    P "round $i end" | tee -a "$O/rounds.txt"
    python3 -I bench/suite_answers.py "$O/round$i.jsonl" >"$O/round$i-answers.txt" 2>&1; echo "suite_answers rc=$?" >>"$O/round$i-answers.txt"
    tail -3 "$O/round$i-answers.txt"
  done
  procs "end" >>"$O/procs.txt"
fi
cmp -s "$BIN" "$SAVED" && echo "binary still the build's" || echo "BINARY CHANGED DURING THE RUN"
P done >>"$O/rounds.txt"
ls -la "$O"
