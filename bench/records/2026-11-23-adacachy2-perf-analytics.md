# Analytics vs SQLite and DuckDB on adacachy2: lane M2 (2026-11-23)

Ada's goal: Amphora about 10x faster than SQLite and DuckDB on analytics, on the same client and
platform, with a production build, multi-threaded. This lane ran the analytic comparison on adacachy2
(runner 00aad8daff2fa3c9daa65d53c425d450, Linux, 32 cores, 122 GiB).

## Verdict, losses first

Amphora is not 10x faster on analytics. On TPC-H SF1 with one connection and one thread each, it is
2.3x slower than DuckDB on one thread and 9.4x slower than DuckDB on every core (geometric means over
the 21 queries that both finished). Against SQLite, tuned as a read-heavy SQLite runs in production,
it is about even: 1.09x faster by the geometric mean, with losses of up to 21x and wins of up to 42x.
That is one pass of 5 runs a query, on a shared machine; see "How far to trust it".

| Amphora (one thread) over | geometric mean of time ratios | queries | left out |
|---|---:|---:|---|
| DuckDB, 32 threads (its default) | 9.43 (slower) | 21 | q05 (Amphora timed out) |
| DuckDB, 1 thread | 2.30 (slower) | 21 | q05 |
| SQLite 3.53.2, mmap on, temp in memory | 0.92 (faster) | 21 | q05 |
| amphora-adaptive (adaptive joins, re-planning) | 0.92 | 21 | q05 |

- **q05 times out on Amphora**, past 60 s on both the counted and the timed run, at this commit.
  SQLite takes 1.73 s, DuckDB 0.12 s, so that is a loss of more than 35x against SQLite. amphora-adaptive
  finishes it in 1.24 s with the right answer. Lane T3 owns this regression.
- **Answers: 22 queries compared, 0 differ** (bench/planner/answers.py over every engine's rows).
  q05's Amphora row is missing because it timed out, so its wrong answer (lane T2's r414) could not
  show up here.
- **Multi-threaded doesn't help Amphora yet.** In parallel mode, Amphora at 8 cores is 0.87x as fast as
  at 1 core (geometric mean, 20 queries). It is 10.7x slower than DuckDB at 8 cores, and q09 times out
  at 8 cores. See "Multi-threaded".
- **JOB:** not run yet. r492 runs it after the TPC-H ladder.

## Per query: Amphora's median and its ratio to each engine

The ratio is Amphora's median over the competitor's. Above 1 means Amphora is slower by that factor.
Medians are over 4 timed runs: of the 5, the first writes its rows under `--answers`, so ratios.py
leaves it out. Rows are sorted by the loss to duckdb-mt.

| query | amphora ms | amphora-adaptive ms | ratio | sqlite ms | ratio | duckdb ms | ratio | duckdb-mt ms | ratio |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| q05 | timed_out | 1236.9 | - | 1727.0 | - | 119.1 | - | 59.4 | - |
| q01 | 1864.8 | 1920.4 | 0.97 | 3348.3 | 0.56 | 185.1 | 10.07 | 31.8 | 58.58 |
| q03 | 1489.3 | 1572.6 | 0.95 | 705.5 | 2.11 | 103.1 | 14.45 | 32.3 | 46.17 |
| q06 | 490.9 | 483.5 | 1.02 | 321.2 | 1.53 | 61.8 | 7.95 | 11.1 | 44.17 |
| q13 | 2188.8 | 1736.7 | 1.26 | 1225.4 | 1.79 | 897.7 | 2.44 | 63.4 | 34.53 |
| q15 | 959.0 | 941.3 | 1.02 | 4734.3 | 0.20 | 472.4 | 2.03 | 29.3 | 32.77 |
| q14 | 585.4 | 509.2 | 1.15 | 4877.1 | 0.12 | 75.1 | 7.80 | 28.1 | 20.84 |
| q09 | 1435.6 | 1503.0 | 0.96 | 1163.0 | 1.23 | 256.9 | 5.59 | 73.9 | 19.42 |
| q07 | 1183.9 | 1132.1 | 1.05 | 7058.8 | 0.17 | 116.9 | 10.13 | 61.4 | 19.28 |
| q12 | 559.7 | 577.1 | 0.97 | 754.0 | 0.74 | 112.9 | 4.96 | 30.3 | 18.49 |
| q19 | 541.0 | 533.7 | 1.01 | 25.5 | 21.25 | 95.8 | 5.65 | 38.8 | 13.95 |
| q21 | 798.2 | 745.9 | 1.07 | 439.2 | 1.82 | 201.2 | 3.97 | 58.9 | 13.54 |
| q18 | 1441.6 | 2686.7 | 0.54 | 448.0 | 3.22 | 37749.5 | 0.04 | 125.6 | 11.48 |
| q04 | 273.7 | 274.6 | 1.00 | 183.4 | 1.49 | 84.2 | 3.25 | 25.9 | 10.58 |
| q10 | 586.7 | 588.7 | 1.00 | 1144.7 | 0.51 | 166.7 | 3.52 | 64.6 | 9.08 |
| q20 | 307.1 | 313.2 | 0.98 | 75.0 | 4.10 | 145.3 | 2.11 | 75.0 | 4.09 |
| q11 | 78.1 | 86.1 | 0.91 | 30.1 | 2.60 | 100.7 | 0.78 | 28.5 | 2.74 |
| q08 | 196.7 | 194.9 | 1.01 | 8224.1 | 0.02 | 119.7 | 1.64 | 73.5 | 2.68 |
| q17 | 77.5 | 80.8 | 0.96 | 46.0 | 1.69 | 90.1 | 0.86 | 32.3 | 2.39 |
| q16 | 134.5 | 149.5 | 0.90 | 103.3 | 1.30 | 321.6 | 0.42 | 60.6 | 2.22 |
| q22 | 65.1 | 232.9 | 0.28 | 33.9 | 1.92 | 96.2 | 0.68 | 74.4 | 0.88 |
| q02 | 25.8 | 26.1 | 0.99 | 163.7 | 0.16 | 51.3 | 0.50 | 50.4 | 0.51 |

Every run's time, its min, max, spread and the load it started at are in
`2026-11-23-adacachy2-perf-analytics/` (rerun `python3 -I spread.py tpch.jsonl drop`).

## What limits the biggest losses

No profile was taken. adacachy2 has no `perf`, and its ptrace scope stops `eu-stack` attaching to a
sibling process. `samply` is installed, so the next lane can profile with it. So each limiter below is
an **inference** from Amphora's recorded plan and row counts (tpch.jsonl, `plan` and `counts`) plus
arithmetic, not a measurement.

1. **q01 (58.6x behind duckdb-mt, 10.1x behind DuckDB on one thread): per-row work on one core.**
   The plan is `SCAN lineitem` plus `USE HASH TABLE FOR GROUP BY`, over 5,916,591 rows counted.
   1864.8 ms / 5.92 M rows = 315 ns a row. DuckDB on one thread takes 185 ms, or 31 ns a row, for the
   same scan and the same 8 aggregates. That fits row-at-a-time decoding and expression evaluation
   against a columnar, vectorized engine. Amphora's vectorized scans (`set_vectorized_scans`, QT.7)
   are off by default, so this run, which uses shipping defaults, leaves them off. DuckDB's other 5.8x
   comes from its 32 threads.
2. **q03 (46x behind duckdb-mt): plan instability, then per-row work.** Amphora's 4 timed runs took
   1425 to 1570 ms after a first run of 988 ms. An isolated run on a reflinked copy of the same database
   took 633.5 ms. That is a 2.5x spread for one query on one build, which fits lane T2's finding that
   plans vary from run to run with learned weights. It was not checked here run by run.
3. **q06 (44x) and q14 (20.8x): one-thread scan cost.** The plan is a scan of lineitem, at 491 ms. q06
   is DuckDB's best case for vectorized filters and 32 threads (11 ms).
4. **q19 (21x behind SQLite): access path.** Amphora scans all of lineitem and probes part by rowid for
   each of the 25,845 rows that pass the filter. SQLite answers in 25 ms, which fits driving from the
   ~120 qualifying parts through lineitem's `(l_partkey, l_suppkey)` index. That is a planner choice,
   not execution speed.

## Each engine's settings

| engine | settings | as production runs it? |
|---|---|---|
| amphora | shipping release profile (thin LTO, 1 codegen unit, overflow checks on), one connection, `cache_size` 2 GiB, one thread, vectorized scans off (Amphora's default), adaptive joins off | yes, except the bench's counting allocator (below) |
| amphora-adaptive | the same, with adaptive joins and re-planning on | a variant of the same engine |
| sqlite | bundled 3.53.2; WAL, `synchronous = NORMAL`, `cache_size` 2 GiB, **`mmap_size` 2,147,418,112** (requested as the i64 maximum; SQLite clamps it to its compiled ceiling, SQLITE_MAX_MMAP_SIZE) and **`temp_store = MEMORY` (2)** | yes, tuned as a read-heavy SQLite. New in this lane (bench commit 29eb13ba, engines.rs) |
| duckdb | bundled v2.0.1, `threads = 1`, `memory_limit` 98.3 GiB (its default, 80% of RAM) | a one-thread baseline |
| duckdb-mt | v2.0.1, `threads = 32` (its default), `memory_limit` its default | yes |

Build: amphora-bench at 29eb13ba (origin/main 364266aa + lane M's f19d9a50 getloadavg fix + the SQLite
tuning), built on adacachy2 with `CARGO_PROFILE_RELEASE_LTO=thin CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS=true` (profile-29eb13ba….txt) in lane S4's target dir, so the two
lanes share one DuckDB build. It took 10 min 57 s. The binary was copied to
~/.cache/amphora-perf-bin/bench-m2/29eb13ba…/ before any timed run.

Data: TPC-H SF1 from dbgen at electrum/tpch-dbgen 32f1c1b9 (bench/planner/tpch/dbgen.sh's revision),
already present in ~/.cache/planner-bench/tpch-sf1 (lineitem.tbl 753,862,072 bytes,
tpch-data.txt). Each engine loaded it into its own database under ~/.cache/amphora-bench-m2/db.

Timeouts: `--timeout-s 60 --count-timeout-s 60`. Both are equal, so a timeout costs 2 minutes, not
lane M's 32. The harness stops a query's runs at the first timeout.

## How far to trust it (the benchmark checklist)

- **Not alternated.** For each query the harness runs each engine in turn: a warming run (Amphora's
  is the counted run), then its 5 timed runs back to back. Engines are seconds apart within a query,
  but drift isn't spread across them. A ratio inside the run-to-run spread is no measured difference.
  Most spreads are 2 to 15%. q03 (Amphora 59%), q11 (SQLite 43%) and q16 (SQLite 39%) are wider.
- **Amphora alone pays a counting allocator.** The bench's global allocator (bench/src/main.rs,
  `Counting`) adds a thread-local add on every Rust allocation over the system allocator, which is
  what Amphora uses in production. SQLite and DuckDB allocate through C and C++ and don't pay it. It
  was not measured this hour. It is a tax on Amphora, so it does not inflate any of Amphora's wins.
- **A shared machine.** Other lanes built and tested on adacachy2 throughout. During the TPC-H phase
  (12:45:21Z to 13:03:40Z, 39 samples 30 s apart) the load was 13.6 to 26.3 (median 18.2) on 32 cores,
  CPU pressure (some, avg10) 0.0 to 3.4% (median 0.5%), I/O pressure some 0.2 to 6.9% (median 3.7%)
  and full 0.1 to 5.1% (median 2.9%). kiln held 16 idle cores for the run. During the build, load
  reached 112 and CPU pressure 38.8%. phases.log has every sample.
- **Errors and work.** Every timed run's status is in tpch.jsonl. The only failure is Amphora's q05
  timeout. answers.py compared every engine's complete answer to every query: 0 differ.
- **DuckDB on one thread, q18: 37.7 s** against 126 ms on 32 threads, a 300x gap. That looks like a
  DuckDB single-thread plan problem (q18's `IN (… GROUP BY … HAVING)`), not a harness error, since
  its answer matches. Amphora's 0.04 ratio against it is DuckDB's problem, not Amphora's win.
- **amphora-adaptive** loses on q22 (3.6x slower than amphora) and q18 (1.9x), and wins on q13 (1.26x)
  and q05 (it finishes).

- **Order check (inconclusive).** A rerun of the four biggest losses (q01, q03, q06, q19) with the order
  reversed (duckdb-mt first, then amphora, 5 runs each) ran on the reflinked copies at about 13:11Z.
  The load had risen to 26 to 54, from 13.6 to 26.3 in the main pass. duckdb-mt's medians stayed close
  to the main pass: 41 against 32 ms on q01, 32 against 39 ms on q19. Amphora's rose 1.6 to 2.1x
  (q01 2954 ms, spread 54%; q03 3136 ms, spread 222%). So this check measured the load, not the order.
  It does say Amphora's one-thread times are far more sensitive to other work on the machine than
  DuckDB's. Why is not known. It isn't CPU: DuckDB runs 32 threads and should suffer more there.
  Memory bandwidth or page-cache residency of the copies are guesses. Raw records: in
  ~/.cache/amphora-bench-m2/out/29eb13ba…/tpch-reversed.jsonl on adacachy2.

## Multi-threaded

**Amphora's parallel execution doesn't scale. From 1 to 8 cores it got slower.** These are medians of 5
runs a point, with Amphora in `ParallelMode::On` (parallel LIMIT, `set_parallel_cores(n)`, the cost
model choosing up to n workers) and duckdb-mt with `SET threads = n`:

| engine | speedup from 1 to 8 cores, geometric mean | queries | range |
|---|---:|---:|---|
| amphora | 0.87 (slower at 8) | 20 | 0.60 to 1.21 |
| duckdb-mt | 2.79 | 21 | 1.09 to 11.05 |

| Amphora over duckdb-mt | geometric mean | queries | left out |
|---|---:|---:|---|
| at 1 core | 2.56 (slower) | 21 | q05 (Amphora timed out) |
| at 8 cores | 10.66 (slower) | 20 | q05, and **q09: Amphora timed out at 8 cores** but took 1246 ms at 1 core |

At 8 cores Amphora's worst losses are q03 (51.8x), q01 (49.1x), q06 (38.7x), q15 (28.5x) and q13
(28.4x). Per query: q01 1840 to 2251 ms, q15 1105 to 1553 ms, q22 249 to 353 ms, while DuckDB's q01
went from 188 to 46 ms. The ladder's records don't say how many workers Amphora's cost model started,
so whether it ran one worker plus coordination overhead or several that contended is **not known**.
The next step is to log the worker count chosen per query and profile q01 at 8 cores (samply).

Caveats: the 1-core point comes from r492's ladder (tpch-cores-partial.jsonl, 13:03Z to 13:08Z). The
8-core point is r603 (tpch-cores8.jsonl, 13:05Z to 13:10Z), run on reflinked copies of the same
databases. kiln wouldn't give 16 idle cores while r492 held 16 (12 were idle), so it got `needs
cpus=8`. The two ran at the same time on one machine, which adds noise to both. The 2, 4 and 16 core
points are still to come from r492.

## Still running (r492 on adacachy2)

r492 (`bash ~/.cache/amphora-perf/bench-m2-run.sh 29eb13ba…`, background, 6 h limit) carries on:
`tpch-cores` (1, 2, 4, 8, 16 cores, amphora vs duckdb-mt), then `job` (113 queries, the five engines,
`--answers`), then `job-cores`. Outputs land in ~/.cache/amphora-bench-m2/out/29eb13ba…/ on
adacachy2: tpch-cores.jsonl, job.jsonl, job-cores.jsonl, answers/, phases.log (load, CPU and I/O
pressure every 30 s and at each phase's start and end). Every phase passes `--resume`, so running the
same script again on the same commit continues where it stopped. To read them:
`python3 -I bench/planner/ratios.py <jsonl>`, `python3 -I bench/planner/answers.py <out>/answers`.

## Files

`2026-11-23-adacachy2-perf-analytics/`: tpch.jsonl (every record), tpch.log, phases.log,
tpch-cores-partial.jsonl (the ladder when this was written: its 1-core point), tpch-cores8.jsonl and tpch-cores8.log (the 8-core point, r603), run.sh (r492's script), spread.py and
phases.py (the spread and pressure tables), the build profile and the data listing.
