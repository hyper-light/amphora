# Lane C3 (planner), 2026-11-24 05:55:48Z to 06:55:48Z: t_pl_8_13, a left join inside a bushy run

Machine: MacBook Pro 2 (runner e30189ea), the only Mac that took a run this hour: load 50 to 86 on 18 cores, each of my
processes at nice 20 on 10 to 16% of a core (CPU time against elapsed, r660). adacachy2 was closed to lanes (Ada's
benchmark window). perf/planner-c3 is main 0101aa38 with this record and no code. Not green, so on branches of their own:
perf/planner-c3-candidate d564a2f0 (the candidate price, the sweep's test, its fixture and generator) and
perf/planner-c3-ab 69dc0668 (main's engine with the probes, and the candidate as c3/candidate.patch).

## 1. Losses first

- **Not merged, and t_pl_8_13 was not reproduced here.** Its only round to run (r570, main's engine, debug, fresh calibration)
  took 2 min 51 s of CPU in 24 minutes and neither failed nor ended before my limit: no expected-against-got rows.
- **The candidate fix has never been run.** Its build (r647) did not end either.
- The sweep was not run. Its test has not been compiled.
- The forced shapes (r636's second half) met my 10 minute limit inside the first shape: no time for either plan.
- r554, r565, r596 and r609, without `on`, were still queued behind other lanes at 06:44Z; r592's sync timed out (60 s).

## 2. Is it wrong for a user? Not where it ran

Earlier lanes' failures are all at the test's plan-shape asserts (`made_bushy > 0`, `in_runs > 0`;
2026-11-23-adacachy2-perf-tpch.md T3 to T10), which stand before its answer checks. None shows a row lost or changed.

Query 0 (`d1 LEFT JOIN d2 s`, no extra ON term), ANALYZEd, main's engine, debug, through SQL; SQLite 3.54.0 answers
`172800,1038240000,1033516800,57600,691200` (115,200 of its 172,800 rows are NULL-extended):

| setting (r660) | bushy | LEFT JOIN in the run | answer |
|---|---|---|---|
| by cost, first run | yes | no | SQLite's |
| by cost, second run | yes | yes | SQLite's |
| LEFT JOIN probes allowed | yes | no | SQLite's |
| adaptive joins | yes | yes | SQLite's |
| parallel mode, 1 core | yes | yes | SQLite's |
| parallel mode, 2 cores | yes | yes | SQLite's |

Query 1 (its mirror FROM), by cost (r636): bushy with the LEFT JOIN in the run, `172800,1032825600,1038240000,57600,345600`, SQLite's.
Whether the parallel hook ran, or the serial loop answered, was not read. Not run: the other six queries, without statistics,
8 workers, the other join methods.

## 3. The trace: why the search does not put the LEFT JOIN in the run (r636, `c3_probe_weighed`)

Query 0 at the weights the test starts with: seek 12.79 rows' visits, hash build 20.77, hash probe 5.87, a row's visit
0.005736 page reads. Every whole plan the search weighed, estimated cost and estimated rows after each table:

| plan | cost | rows |
|---|---|---|
| loops d1, s; run (d2, f2); probe f1 (the mirror) | 5,300, taken | 6, 8.2, 1, 237, 230,498 |
| loops d2; run (d1, s, f1); probe f2 (the test's plan) | 14,095 | 1, 6, 8.2, 1,946, 230,498 |
| left-deep d2, f2, f1, d1, s | 130,452 | 1, 237, 1,421,655, 168,508, 230,498 |

- The run (d1, s, f1) reads f1 by an automatic index (EXPLAIN: `SEARCH f1 USING COVERING INDEX ephemeral_f1_t3 (d1=?)`).
- By the formula (`estimate_ephemeral_index_build_cost`, `n * log2(n) * cpu_cost_per_seek`) that index is priced at
  12,000 x 13.55 x 12.79 x 0.005736 = 11,930, which is 85% of the plan's 14,095. I computed it; the trace does not log it.
- The formula is the Turso import's (f2e07044), where a seek's price stood for a comparison's. Since 2996f99c
  `cpu_cost_per_seek` is the learned price of a whole search (calibration times a binary search of a page's 512 keys), so each
  insert is priced at 13.55 searches. **Inferred, not measured: that an insert really takes about one.**
- With an insert at one search and one visit of its row, the index is 949 and the plan 3,114, under the mirror's 5,300.
- The query's first run moved the hash probe's weight from 5.87 to 88.0 (then 312.9 after query 1). At 88.0 the mirror costs
  51,705 and the test's plan 19,749, so the second run has the LEFT JOIN in the run. The test passes where that weight moves
  far enough and fails where it does not. Why one run moves it 15 times is not traced.

## 4. The candidate (perf/planner-c3-candidate d564a2f0, never run)

A row's insert into an ephemeral index costs a seek and a row's visit. `t_qt_5_5_an_ephemeral_index_past_its_cache_reads_and_writes_its_leaves`
restated the old formula and is changed with it; its page counts are not. The price reaches every automatic index, so TPC-H's 22 plans have to
be listed before and after.

## 5. What is ready to run

- `JOBS=16 bash c3/all.sh` in a checkout of perf/planner-c3-ab (a Mac; r596 is it, queued) builds both arms and prints, for each: six rounds of
  t_pl_8_13 on fresh calibrations, the plans weighed, both shapes forced and timed (15 runs), an automatic index's build timed at 1,500 to
  48,000 rows against a scan and its price, eight queries under 13 settings judged by sqlite3, and the 420-query sweep.
- On perf/planner-c3-candidate, bench/sqlite_outer_joins.py writes crates/engine/tests/fixtures/outer_joins.tsv: 400 drawn queries of 4 and 5 tables at three sizes and 20 over the
  test's facts (302 hold a LEFT JOIN, 134 a RIGHT, 137 a FULL, 30 give no row), with SQLite 3.54.0's answers.
- `t_pl_8_13_outer_joins_answer_as_sqlite_whatever_the_plan` holds every plan to them.
