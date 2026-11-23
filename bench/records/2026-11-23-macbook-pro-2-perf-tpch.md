# Lane T (tpch), hour 1: TPC-H q05's loss is a regression after 2026-10-29, not a tuning gap

Lane M's record (2026-11-23-mac-studio-d1323898-perf-analytics.md) found q05 at SF1 unfinished in
1800 s on amphora and amphora-adaptive at head 2d939193, against SQLite 1.73 s and DuckDB (one
thread) 0.10 s. This hour sets the regression window and leaves the SF0.1 reproduction queued for
the successor. No fix and no sweep yet.

## The same query ran in 0.64 s at 26657fdf

`bench/records/2026-10-26-jupiter-planner-tpch-sf1-e.jsonl` (jupiter, Windows, 32 cores, commit
26657fdf, 2026-10-29, `--runs 3`), q05 at SF1, one thread each:

| engine | runs (ms) |
|---|---|
| amphora | 660.7, 645.2, 636.1 |
| sqlite | 3436.9, 3343.2, 3350.1 |
| duckdb (1 thread) | 241.5, 220.8, 221.9 |
| duckdb-mt | 103.1, 101.7, 100.2 |

Amphora then beat SQLite by 5.3x. Its plan (EXPLAIN QUERY PLAN FORMAT=JSON in that record), with
the rows each step counted:

| step | access | estimate | rows counted |
|---|---|---|---|
| orders | SCAN (date range as filter) | 7,500 | 227,597 |
| customer | SEARCH rowid=? | 7,500 | 227,597 |
| nation | SEARCH rowid=? | 7,500 | 227,597 |
| region | SEARCH rowid=? | 750 | 46,008 |
| lineitem | SEARCH sqlite_autoindex_lineitem_1 (l_orderkey=?) | 3,750 | 184,082 |
| supplier | SEARCH rowid=? (s_nationkey = c_nationkey checked on the row) | 6 | 7,243 |

The largest intermediate result is 227,597 rows: no cross product, every join on a key. The
cycle `c_nationkey = s_nationkey` is checked on the supplier row the `l_suppkey = s_suppkey`
lookup found.

## The window

- `git log 2d939193..origin/main -- crates/engine/translate crates/engine/vdbe` is empty: current
  main (28fa609c) plans and runs q05 as 2d939193 did, so lane M's loss stands on main.
- `git log --no-merges 26657fdf..2d939193 -- crates/engine/translate/optimizer
  crates/engine/translate/main_loop.rs` holds 165 commits. Those touching the join search and
  estimates include the bushy plans (PL.8, aca73004 and after), the degree-sequence bounds (PL.11,
  ec0bf7ba), a seek and a hash join weighed by cost alone with the `rhs_has_selective_seek` gate
  removed (QT.5.8, 9422adcb), pages priced by cache misses (c3e83e5f), and no table read whole
  beneath loops (PL.6, 2084ac96). None is named as the cause yet: that needs the new plan.

## Reproduction (queued, not finished this hour)

On the MacBook Pro 2 (runner e30189ea, `needs macos/arm64,cpus=4,mem=16g`, load average 34 to 61
on 18 cores during the hour), in `~/.cache/amphora-perf/tpch-remote` at origin/main 28fa609c:

- TPC-H SF0.1 is generated at `~/.cache/planner-bench/tpch-sf0.1` (dbgen 32f1c1b9; lineitem
  600,572 rows, orders 150,000, customer 15,000, supplier 1,000).
- `amphora-bench` (bench workspace, release) was still compiling bundled DuckDB after 45 minutes
  at 4 jobs on the loaded machine; target dir `~/.cache/amphora-perf-target/tpch`.
- The q05 run is queued behind it: `q.sh origin/main q05 4 300 q05-main` (lane T's script, kept at
  `~/.cache/amphora-perf/tpch-scripts/q.sh` on adacachy and the Mac). It runs the planner bench
  with `--targets amphora,sqlite --runs 4 --timeout-s 300 --count-timeout-s 300 --answers`, prints
  Amphora's plan and the rows each step counted beside SQLite's EXPLAIN QUERY PLAN, and compares
  answers. Its output lands in `~/.cache/amphora-perf/tpch-out/q05-main/` on the Mac.

It reproduces at SF0.1 (inferred from the run, not yet read from a record): run r414 at origin/main
78fa9fdb loaded both engines (SQLite's lineitem in 3.0 s, its indexes and statistics in 2.3 s) and
Amphora's counted q05 was still running 4 min 43 s into the process at 05:43Z. The October plan's
work at SF0.1 is about a tenth of SF1's 0.64 s. The record (plan and the rows counted at the
300 s deadline) lands in `~/.cache/amphora-perf/tpch-out/q05-main/rec.jsonl` on the Mac when r414
ends.

## Next

1. Read q05's plan and counts at SF0.1 from that run against the October plan above. The step
   whose rows explode names the cause.
2. If the plan only goes wrong at SF1, rerun at SF1 with `--count-timeout-s` short and read the
   plan alone (`record["plan"]` is written before the counted run).
3. Bisect the 165 commits by plan shape, not time: EXPLAIN QUERY PLAN needs no run.
