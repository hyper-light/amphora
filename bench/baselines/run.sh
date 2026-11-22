#!/usr/bin/env bash
# Baseline numbers before optimizing (goal 63c98840; bench/baselines/README.md): each number from one
# command, its records in $OUT with the machine, the commit and the host's load, then its summary
# (check.py). A part's harness is the one the number already had; this script names its inputs and
# where its records go.
#
# usage: bench/baselines/run.sh <part> [arguments for the part's harness...]
#   job-plan     JOB's planning time per query on Amphora; exits 1 if any query plans in over 20 ms
#   job-search   JOB's join search time per query, within its planning
#   join-kinds   each join kind's time at each build size (bench/join_methods; arguments: rounds, sizes)
#   point-reads  single-row reads per class on every row store, Amphora's p50 over the leader's
#   scan-agg     whole-table scans and aggregates in ns a row: the suite's on the row stores and
#                ClickHouse, TPC-H's q01 and q06 on Amphora, SQLite and DuckDB
# env: BIN     amphora-bench (default ${CARGO_TARGET_DIR:-target}/release/amphora-bench)
#      DATA    the planner bench's data: imdb/ and tpch-sf1/ (default ~/.cache/planner-bench)
#      JOB     a clone of gregrahn/join-order-benchmark (default ~/.cache/research-src/join-order-benchmark)
#      OUT     where records go (default bench/records)
#      RUNS    timed runs a query (default 3); TARGETS: the part's engines (each part has its own)
#      SERVERS point-reads' and scan-agg's local servers (default postgres,mysql; scan-agg's adds
#              clickhouse), run after TARGETS as a group of their own, in their containers
#
# Fair comparisons (Ada, 2026-11-21): every engine set beside another ran through the same driver
# (amphora-bench, one timing method) on the same host, disk and filesystem, and all of them in the
# driver's process or all as local servers, containers for all or none. Amphora runs only in its
# driver's process (it carries no server of its own), so TARGETS are the engines that run there too,
# and SERVERS (PostgreSQL, MySQL, ClickHouse, each in its container) are a group of their own, in a
# file of their own: check.py sets an engine against Amphora only from Amphora's own run and
# platform, as each record says them (its `client` and `platform`).
set -euo pipefail
part=${1:?part: job-plan, job-search, join-kinds, point-reads or scan-agg}
shift
here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
bench=$(cd -- "$here/.." && pwd)
bin=${BIN:-${CARGO_TARGET_DIR:-$bench/../target}/release/amphora-bench}
data=${DATA:-$HOME/.cache/planner-bench}
job=${JOB:-$HOME/.cache/research-src/join-order-benchmark}
out=${OUT:-$bench/records}
runs=${RUNS:-3}
mkdir -p "$out"
# One run a file (to the second), so the leader is always from the same run.
stem="$out/$(date -u +%Y-%m-%dT%H%M%SZ)-$(hostname -s)-baseline-$part"
# Native Windows programs take Windows paths; elsewhere a path is itself.
native() { if command -v cygpath >/dev/null; then cygpath -w "$1"; else echo "$1"; fi; }
check() { python3 -I "$here/check.py" "$@"; }

case "$part" in
  job-plan | job-search)
    # One process loads (`--only -` names no query), another times: planner/README.md says why.
    args=(--planner --bench job --data "$(native "$data/imdb")" --queries "$(native "$job")" --dir "$(native "$data/db-job")" --targets "${TARGETS:-amphora}")
    "$bin" "${args[@]}" --only - --out "$(native "$stem-load.jsonl")"
    "$bin" "${args[@]}" --runs "$runs" --out "$(native "$stem.jsonl")" "$@"
    check "${part#job-}" "$stem.jsonl"
    ;;
  join-kinds)
    bash "$bench/join_methods/run.sh" "$stem" "$@"
    cat "$stem/host.txt"
    ;;
  point-reads)
    only='point_pk$,point_pk_one_column$,point_secondary$,point_text$,json_field$,blob_read$,text_read$'
    SUITE_OUT="$stem.jsonl" bash "$bench/suite.sh" "${TARGETS:-amphora,sqlite,turso}" --only "$only" --clients 1 "$@"
    servers=${SERVERS-postgres,mysql}
    if [[ -n $servers ]]; then
      SUITE_OUT="$stem-servers.jsonl" bash "$bench/suite.sh" "$servers" --only "$only" --clients 1 "$@"
    fi
    check leader "$stem"*.jsonl
    ;;
  scan-agg)
    only='filter_no_index$,like_substring$,table_count$,table_group_by$'
    SUITE_OUT="$stem.jsonl" bash "$bench/suite.sh" "${TARGETS:-amphora,sqlite,turso}" --only "$only" --clients 1 "$@"
    servers=${SERVERS-postgres,mysql,clickhouse}
    if [[ -n $servers ]]; then
      SUITE_OUT="$stem-servers.jsonl" bash "$bench/suite.sh" "$servers" --only "$only" --clients 1 "$@"
    fi
    tpch=(--planner --bench tpch --sf "${SF:-1}" --data "$(native "$data/tpch-sf${SF:-1}")" --queries "$(native "$bench/planner/tpch/queries")" \
      --dir "$(native "$data/db-tpch-sf${SF:-1}")" --targets "${TPCH_TARGETS:-amphora,sqlite,duckdb}")
    # The load's records (lineitem's rows among them) are written by the process that loads, once a
    # database: they are kept beside it for every later pass.
    loaded="$data/db-tpch-sf${SF:-1}-load.jsonl"
    if [[ ! -s $loaded ]]; then
      "$bin" "${tpch[@]}" --only - --out "$(native "$loaded")"
    fi
    "$bin" "${tpch[@]}" --only q01,q06 --runs "$runs" --out "$(native "$stem-tpch.jsonl")"
    check scan "$stem"*.jsonl "$loaded"
    ;;
  *) echo "no part $part" >&2; exit 2 ;;
esac
echo "records: $stem*" >&2
