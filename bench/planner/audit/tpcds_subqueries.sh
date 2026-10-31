#!/usr/bin/env bash
# The planner audit's row 10 on TPC-DS: EXPLAIN QUERY PLAN of each TPC-DS query holding a
# correlated subquery, on TPC-DS's schema (no rows, no indexes), through `amphora-bench --eqp`. A
# semi- or anti-join shows as a join in the plan; a subquery kept shows its tables after the
# outer loops (an EXISTS) or as a CORRELATED SCALAR SUBQUERY. The queries and
# schema are read from a TPC-DS query set (turso's perf/tpc-ds at fbcfa43), not kept here.
#
#   bench/planner/audit/tpcds_subqueries.sh BENCH_BINARY TPCDS_DIR > bench/planner/audit/tpcds-subqueries.out
set -euo pipefail
bin="${1:?the amphora-bench binary}"
dir="${2:?a TPC-DS directory holding schema.sql and queries/NN.sql}"
scratch="${AUDIT_SCRATCH:-$HOME/.cache/planner-audit}/tpcds"
mkdir -p "$scratch"
{
  cat "$dir/schema.sql"
  for q in 01 06 10 16 30 32 35 41 69 81 92 94; do
    echo "-- TPC-DS Q$q"
    tr '\n' ' ' < "$dir/queries/$q.sql" | sed -e 's/;[[:space:]]*$//' -e 's/^/EXPLAIN QUERY PLAN /' -e 's/$/;/'
    echo
  done
} > "$scratch/script.sql"
"$bin" --eqp --script "$scratch/script.sql" --dir "$scratch/db" | sed -n '/^> EXPLAIN/,$p'
