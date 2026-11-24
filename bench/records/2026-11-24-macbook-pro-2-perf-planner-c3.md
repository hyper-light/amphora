# Lane C3 (planner), 2026-11-24 05:55:48Z to 06:55:48Z: t_pl_8_13, a left join inside a bushy run

Machine: MacBook Pro 2 (runner e30189ea), the only Mac that took a run this hour, load 50 to 77 on 18 cores; kiln runs
its commands there at nice 20, and each of mine got 10 to 16% of a core (a process's CPU time against its elapsed, r660).
adacachy2 was closed to lanes (Ada's benchmark window). Branch perf/planner-c3, from origin/main 0101aa38. Probes:
branch probe/planner-c3 (test code only over main until 6bba9bd6, which adds the candidate), clone
~/.cache/amphora-perf/planner-c3-probe, pushed as perf/planner-c3-probe.

## 1. What failed, losses first

- **Not merged.** The candidate fix and the sweep below are built but no run of theirs finished.
- The failure recorded by every earlier lane is at the test's plan-shape asserts (`made_bushy > 0`, `in_runs > 0`),
  which stand before its answer checks. No record shows a row lost or changed.
- r554, r565, r596, r609 (no `on`) were queued the whole hour behind other lanes; r592's sync timed out (60 s).

## 2. Is it wrong for a user?

- Query 0 and query 1 of the test through SQL as a user runs it (default settings, ANALYZEd, one worker; r636, the
  engine of main 0101aa38): `172800,1038240000,1033516800,57600,691200` and
  `172800,1032825600,1038240000,57600,345600`. SQLite 3.54.0 gives both answers (bench/sqlite_outer_joins.py). Query 0
  ran with the LEFT JOIN outside the run, query 1 with it inside: 115,200 of its 172,800 rows are NULL-extended.
- The other six queries, no statistics, several workers, and the 420-query sweep: not run to an end (section 5).

## 3. The trace (r636: `c3_probe_weighed`, every whole plan the search weighed for query 0)

Weights at the test's start: seek 12.79 rows' visits, hash build 20.77, hash probe 5.87, a row's visit 0.005736 pages.

| plan | estimated cost | |
|---|---|---|
| mirror: loops d1, s; run (d2, f2); probe f1 | 5,300 | taken, LEFT JOIN outside the run |
| loops d2; run (d1, s, f1); probe f2 | 14,095 | the test's plan |
| left-deep d2, f2, f1, d1, s | 130,452 | |

- The run (d1, s, f1) reads f1 by an automatic index (`SEARCH f1 USING COVERING INDEX ephemeral_f1_t3 (d1=?)`): a hash join
  there has no key with the table before it, s, and PL.6 refuses a whole read under d1's rows.
- `estimate_ephemeral_index_build_cost` prices that index at `n * log2(n) * cpu_cost_per_seek`: 12,000 x 13.55 x 12.79 x
  0.005736 = 11,930 of the plan's 14,095. Its per-comparison price dates from the Turso import (f2e07044); since
  2996f99c `cpu_cost_per_seek` is the learned price of a whole search, its comparisons in it (calibration's seek is a
  binary search of a page's 512 keys).
- At an insert priced as one search and one visit of its row the index is 949 and the plan 3,114: under the mirror's
  5,300, and under it again at the weights the query's first run leaves (8,768 against 51,705).
- The first run moved the hash probe's weight from 5.87 to 88.0 (q0) and 312.9 (q1), which is what makes the test pass where it
  passes: at 88 the mirror costs 51,705 and the test's plan 19,749. The two plans' order turns on a number one run under
  load can move 50 times.

## 4. The candidate (perf/planner-c3, not merged)

`estimate_ephemeral_index_build_cost`: a row's insert is a seek and a row's visit. The restated formula in
`t_qt_5_5_an_ephemeral_index_past_its_cache_reads_and_writes_its_leaves` follows it; its page counts are unchanged.
**Not shown:** that a real build costs that. `c3_probe_autoindex` times it at 1,500 to 48,000 rows against a scan, and
`c3_probe_forced` times the two shapes; both unfinished.

## 5. The sweep (not run)

bench/sqlite_outer_joins.py draws 400 queries of 4 and 5 tables (302 with a LEFT JOIN, 134 RIGHT, 137 FULL, 30 of 420 empty) at
three sizes and 20 over the test's facts, and writes SQLite 3.54.0's answers to crates/engine/tests/fixtures/outer_joins.tsv.
`t_pl_8_13_outer_joins_answer_as_sqlite_whatever_the_plan` holds every plan to them, with and without statistics, at one
and eight workers, with LEFT JOIN probes, under three pinned weights and as hand-built bushy trees. Not compiled.
