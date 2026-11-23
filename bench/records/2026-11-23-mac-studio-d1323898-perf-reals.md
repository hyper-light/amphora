# Lane R1 (reals), 2026-11-23 18:47Z to 19:46Z: the SUM/AVG-over-REAL A/B harness; reference answers; no engine answers or timings yet

Ada's goal: Amphora about 10x faster than DuckDB and SQLite on analytics and TPC-H, its answers
following each dialect's docs exactly (RULES 5f). SUM and AVG over REAL keep q01 and q06 on one
thread. Ada decides by an A/B between arm (a) `RealSums::RowOrder` (G9, 1b0543f6) and arm (b)
`RealSums::Exact` (G8, 378fe9ed), against today's `Serial`. The default stays `Serial`.

## Verdict, losses first

- **No engine answers and no timings yet.** r1438 spent 4.5 min in a `find` over the Mac's
  `~/.cache` and was stopped. r1455 built (4 min 38 s) and loaded SF1 (1 min 42 s). Its first
  answers call then failed: `AMPHORA_BENCH_CORES`, applied at open, put the connection in parallel
  mode before the bench's load check, `SELECT 1 FROM bench_loaded LIMIT 1` found no row, and the
  bench dropped and reloaded every table, then failed creating `bench_loaded`. Fixed (fe02d165:
  the setting applies after the check). The rerun is r1468 on Mac Studio 2 (d1323898), phases
  `build answers timed-a answers-b timed-b`, outputs in `~/.cache/amphora-perf/reals-out/sf1/` there.
- **Side finding (not chased, rule 0):** under `ParallelMode::On` with parallel LIMIT and a core
  cap, `SELECT 1 FROM bench_loaded LIMIT 1` on a one-row table did not return its row (or errored)
  in r1455. Worth a test in the engine's morsel suite.
- **SF10 not started.** Its dbgen and load take about 10x SF1's.
- **The bench crate is not clippy-clean on main.** `cargo clippy --manifest-path bench/Cargo.toml
  --all-targets -- -D warnings` reports 166 error lines on fe02d165 (r1469), none in the lines this
  lane touched (`planner/mod.rs` 434-441). Examples: `items_after_test_module` and
  `disallowed_methods` (`std::env::temp_dir`) in `bench/src/scale.rs`. The planner tests pass
  (17 of 17 on c42135b3, r1457).
- **These hard cases do not separate KBN from the exact sum.** On all 29 columns of the reference, SQLite's
  KBN in row order equals the exact sum rounded once. So Serial, RowOrder and Exact should agree on
  them unless Amphora's code departs from its algorithm. A case that breaks KBN (several magnitudes
  cancelling at once) is still to add.

## Unit 1: what each algorithm answers on the real rows (bench/real_sums_ref.py, done)

The reference reads `lineitem.tbl` (dbgen 32f1c1b9, SF1, 6,001,215 rows) in row order, the order
both engines scan, and computes each column three ways:

- **SQLite.** `sumStep`, `sumFinalize`, `avgFinalize` and `totalFinalize` from sqlite.org's
  `src/func.c` (trunk, read 2026-11-23). Lines 1925-1931 name the algorithm: "Do one step of the
  Kahan-Babushka-Neumaier summation". `kahanBabuskaNeumaierStep` is at 1932, the integer split at
  1949 and 1964, `sumStep` at 1988, and `sumFinalize` at 2062 (`p->rSum+p->rErr` unless the error
  term overflowed, line 2070). `avgFinalize` (2079) divides that by the count (2090). The release
  notes don't name the algorithm. 3.43.1 (2023-09-11) says: "Fix a regression in the way that the
  sum(), avg(), and total() aggregate functions handle infinities". A NaN result reads NULL.
- **PostgreSQL 19.0.** From `~/Projects/postgres` at 5c839a2a08, "Stamp 19.0".
  - `pg_aggregate.dat` 72-73: `sum(float8)` folds `float8pl` from its first input, with no initial
    value, and combines with `float8pl`.
  - `float.h` 116-126, `float8_pl_safe`: plain `+`; an infinite result from finite inputs is an
    overflow error.
  - `pg_aggregate.dat` 43-45: `avg(float8)` is `float8_accum` from `{0,0,0}`.
  - `float.c` 3090-3160, `float8_accum`: Youngs-Cramer, `Sx += newval`. If `Sx` or `Sxx` turns
    infinite from finite inputs, that is an overflow error.
  - `float.c` 3254-3270: `Sx / N`.
- **Exact.** The exact sum, rounded once. AVG divides that by the count.

| query | column | SQLite (func.c) | PostgreSQL 19 (float.c) | exact |
|---|---|---|---|---|
| q01 | sum(l_discount) | 300057.33 | 300057.33000053867 | 300057.33 |
| q01 | avg(l_discount) | 0.04999943011540164 | 0.04999943011549139 | 0.04999943011540164 |
| q01 | sum(price*(1-disc)) | 218102223885.0001 | 218102223884.98923 | 218102223885.0001 |
| q01 | avg(l_extendedprice) | 38255.13848465686 | 38255.13848465641 | 38255.13848465686 |
| q02 | sum(1e16, -1e16, 1.0) | 2.1417200000000031e+21 | 2.1417199999898327e+21 | 2.1417200000000031e+21 |
| q02 | avg | 356881064917688.0 | 356881064915993.3 | 356881064917688.0 |
| q03 | sum(0.1, 0.2, -0.3, 0.0) | 85747.40000000004 | 85747.39999709961 | 85747.40000000004 |
| q03 | avg | 0.014288339944494579 | 0.014288339944011273 | 0.014288339944494579 |
| q04 | sum(1e300, 1e-300, -1e300, 5e-324) | 4.28606e+305 | 4.286059999982923e+305 | 4.28606e+305 |
| q04 | avg | 7.141987080949441e+298 | ERROR: overflow (Sxx) | 7.141987080949441e+298 |
| q05 | sum(-0.0) | 0.0 | -0.0 | 0.0 |
| q05 | avg(-0.0) | 0.0 | 0.0 | 0.0 |
| q06 | sum(one +inf) | inf | inf | inf |
| q06 | sum(+inf and -inf) | NULL | NaN | NULL |
| q07 | sum(1e308 each row) | inf | ERROR: overflow | inf |
| q07 | avg(1e308 each row) | inf | ERROR: overflow | inf |
| q07 | sum(+1e308 +1e308 -1e308 -1e308) | inf | ERROR: overflow | inf |
| q08 | sum(2^53+1 among 0.5) | 1.3510798882111492e+22 | 1.3510798882111488e+22 | 1.3510798882111492e+22 |
| q08 | avg | 2251343916542149.0 | 2251343916542148.2 | 2251343916542149.0 |

`total()` equals `sum()` in every row (SQLite only; PostgreSQL has none). NaN can't be an input in
the SQLite dialect. floatingpoint.html lists "NaN" among a binary-64's values, but SQLite reads a
NaN result as NULL. So NaN is a Postgres-dialect case, and PostgreSQL carries it through `+`.

What this settles before any engine run:

- **PostgreSQL 19.** No arm can match it. Each arm, Serial included, is a compensated or exact sum
  that starts at +0.0 and reads overflow as infinity. PostgreSQL adds plainly in row order, starts
  `sum` from the first input (so `sum(-0.0)` is -0.0), errors on overflow, and keeps NaN. Its
  answer differs from the exact sum in 17 of the 21 columns where it applies. Main has no Postgres
  dialect for SUM to route through: `crates/engine/dialect/` holds only `sqlite.rs`, and
  `crates/pg-parser` is used by no other crate. A Postgres dialect would need its own
  `float8pl`/`float8_accum` aggregate under any arm. Its parallel plan would combine partial sums
  as PostgreSQL does (`aggcombinefn`), so its answer would depend on the plan, as PostgreSQL's does.
- **SQLite.** On these rows, SQLite's KBN equals the exact sum. Exact can differ from SQLite only
  where KBN is not exact. Serial and RowOrder can differ only where Amphora's KBN departs from
  `func.c`'s. r1468's `answers` phase measures both.

## Unit 2: speed (not run)

r1468 runs it. `timed-a` alternates Serial, Exact, RowOrder and DuckDB (`duckdb-mt`,
`SET threads = n`) over 5 rounds, rotating which goes first. Each round runs q01 and q06 on the
1/4/8/16 ladder, 5 runs each, and every run records its load. `timed-b` runs the other
REAL-summing queries (q03 q05 q07 q08 q09 q10 q11 q14 q15 q17 q19 q22) for one round of 3 runs.
Every arm runs from one binary, so every arm plans from one calibration (RULES 5e: one executable,
one key). A p99.9 over 25 runs is the slowest run. `bench/real_sums_stats.py` prints the run
count beside every percentile.

## Successor: commands

On Mac Studio 2 (d1323898), `needs macos/arm64,cpus=16,mem=12g`, `on` that runner (the build, the
data and the database are there), sync from a clone of `perf/reals`:

```
bench/real_sums_ab.sh 1 $HOME/.cache/amphora-perf/reals-out/sf1 build answers timed-a answers-b timed-b   # r1468
python3 -I bench/real_sums_answers.py ~/.cache/amphora-perf/reals-out/sf1   # unit 1 table: arms x workers vs Serial, SQLite, references
python3 -I bench/real_sums_stats.py ~/.cache/amphora-perf/reals-out/sf1     # unit 2 table: p50/p99/p99.9, ratios vs Serial and DuckDB
bench/real_sums_ab.sh 10 $HOME/.cache/amphora-perf/reals-out/sf10 data ref build load answers timed-a   # SF10
```

# Lane R2 (reals), 2026-11-23 19:56Z to 20:51Z: SF1 answers and q01/q06 timings from r1468; KBN's loss case built and referenced, its engine answers not yet run

Ada's goal: Amphora about 10x faster than DuckDB and SQLite on TPC-H, its answers following each
dialect's docs exactly. SUM and AVG over REAL keep q01 and q06 on one thread. Ada picks arm (a)
`RowOrder` or arm (b) `Exact` from this A/B. The default stays `Serial`.

## Verdict, losses first

- **Not done in this hour.**
  - The engine's answers on KBN's loss case (real_sums q09). The case, its references and the
    command are ready. It needs r1468's database, which r1468 holds until it ends.
  - The twelve other REAL-summing TPC-H queries at 1, 4 and 8 workers. r1468 answers them at 16
    only. Their timings are r1468's `timed-b`, still running when this was written.
  - PostgreSQL 19 references for those twelve. Their joins feed SUM in plan order, so PostgreSQL's
    own answer depends on its plan. None computed.
- **Exact (arm b) answers NULL where a sum overflows.** On R1's q07 (`sum(1e308)` over 6,001,215
  rows, and `+1e308 +1e308 -1e308 -1e308` within each order), Exact returns NULL at 1, 4, 8 and 16
  workers. SQLite 3.53.2, func.c's steps and the exact sum all say `inf`. This is a bug in G8's arm,
  not a property of exact summation.
- **Both arms are slower than Serial on one worker.** q01: Exact 3.0x, RowOrder 3.3x slower. q06:
  8.5x and 8.7x slower. On q06 only 114,160 rows reach the sum, so the cost is likely the morsel
  path's per-row work, not the sum (a guess: not profiled).
- **RowOrder gets slower as workers are added on q01.** 6.8 s at 1 worker, 4.4 s at 4, 9.3 s at 8,
  23.8 s at 16. Serial takes 2.07 s at every worker count.
- **Neither arm comes near DuckDB.** At 16 workers DuckDB answers q01 in 26 ms and q06 in 6.1 ms.
  That is 25x faster than Exact on q01 (654 ms) and 64x on q06 (389 ms). The 10x goal needs the
  per-row cost cut whichever arm wins.
- **R1's side finding reproduces in isolation, and LIMIT is not its cause** (below).
- **Mac Studio 2 was shared.** Load was 7 to 20 on 32 cores during the timed rounds (other lanes'
  work). The arms alternate within each round, so each saw the same spread.

## Answers at SF1

The arms run from one binary (fe02d165, r1468), at 1, 4, 8 and 16 workers. "SQLite 3.53.2" is the
bench's own sqlite target on the same data and statement. The references are bench/real_sums_ref.py
over `lineitem.tbl` in row order (R1's ref.json, and r1515's ref-r2.json for q09 and TPC-H q01, q06).

| case | arm | bit-identical to Serial | equal to SQLite's KBN sum | equal to PostgreSQL 19's sum |
|---|---|---|---|---|
| R1's hard cases q01-q08, 29 columns | Serial | yes at 4, 8, 16 workers | yes, all 29 | 4 of 21 (8 are `total`, which PostgreSQL lacks) |
| same | RowOrder | yes at 1, 4, 8, 16 | yes, all 29 | 4 of 21 |
| same | Exact | 25 of 29: q07's 4 columns are NULL, Serial's `inf` | 25 of 29 | 4 of 21 |
| KBN's loss, q09, 3 columns | all | not run yet | not run yet | not run yet |
| TPC-H q01, 4 groups x 7 REAL cells | all three | yes at 1, 4, 8, 16 | yes, all 28 | 8 of 28 (20 differ, up to 20,675 ulps) |
| TPC-H q06, 1 cell | all three | yes at 1, 4, 8, 16 | yes | no (71 ulps) |
| TPC-H q03 q05 q07 q08 q09 q10 q11 | all three | yes at 16 | yes at 16 | no reference |
| TPC-H q14 q15 q17 q19 q22 | all three | yes at 16 | SQLite's answers were not written yet | no reference |

On every case measured, SQLite's KBN sum equals the exact sum rounded once. So these cases cannot
tell Exact from SQLite except by Exact's overflow bug. q09 is the case that can.

### KBN's loss case (bench/planner/real_sums/queries/q09.sql)

KBN keeps one error term. It loses when that term has to hold two magnitudes at once. The inputs
are 1e36, 1e18, 1.0, -1e36 and -1e18 on line 1 of orders 1, 1500000, 3000000, 4500000 and 6000000,
a quarter of the table apart, so they land in different workers' morsels. Every other row is 0.0.
r1515 counted exactly 5 nonzero inputs in `lineitem.tbl`.

- 1e18 is below half an ulp of 1e36, so it goes to the error term.
- Then 1.0 is below half an ulp of 1e18 (128), so the error term drops it.
- The two big values cancel, and -1e18 cancels the error term.

| column | SQLite (func.c steps) | PostgreSQL 19 (float.c) | exact |
|---|---|---|---|
| sum | 0.0 | -1e+18 | 1.0 |
| avg | 0.0 | -166632923499.658 | 1.6663292349965798e-07 |
| total | 0.0 | n/a | 1.0 |

Expected from the code, not yet measured: Serial and RowOrder answer 0.0 (SQLite's), Exact 1.0.

### Full answer tables (python3 -I bench/real_sums_answers.py on r1468's output, 20:22Z)

Hard cases: every cell of Exact and RowOrder equals Serial at 1 worker, at every worker count,
except Exact on q07 (`NULL at all`). SQLite 3.53.2 equals SQLite's func.c reference on all 29.

| query | column | Serial | Exact | SQLite 3.53.2 | PostgreSQL 19 ref |
|---|---|---|---|---|---|
| q07 | sum(1e308 each row) | inf | NULL | inf | ERROR: value out of range: overflow |
| q07 | avg(1e308 each row) | inf | NULL | inf | ERROR: value out of range: overflow |
| q07 | total(1e308 each row) | inf | NULL | inf | n/a |
| q07 | sum(+1e308 +1e308 -1e308 -1e308) | inf | NULL | inf | ERROR: value out of range: overflow |

TPC-H cells that differ by bits (most ulps). Each arm at each worker count, and SQLite 3.53.2,
gives the same row:

| query | vs Serial at 1 | vs SQLite 3.53.2 | vs SQLite ref (func.c) | vs PostgreSQL 19 ref | vs exact ref |
|---|---|---|---|---|---|
| q01 | 0 (0) | 0 (0) | 0 (0) | 20 of 28 (20,675) | 0 (0) |
| q06 | 0 (0) | 0 (0) | 0 (0) | 1 of 1 (71) | 0 (0) |

## Timings at SF1 (r1468 `timed-a`, 19:47Z to 20:19Z)

25 runs per cell: 5 rounds alternating Serial, Exact, RowOrder and DuckDB, rotating which goes
first, 5 runs each. 0 failed. With 25 runs, p99 is the slowest run. ms; ranges are min to max.

| query | workers | Serial p50 / p99 | Exact p50 / p99 | RowOrder p50 / p99 | DuckDB p50 / p99 |
|---|---|---|---|---|---|
| q01 | 1 | 2070 / 2432 | 6171 / 6953 | 6773 / 7329 | 220 / 292 |
| q01 | 4 | 2067 / 2119 | 1880 / 2088 | 4432 / 4909 | 68.5 / 73.8 |
| q01 | 8 | 2066 / 2120 | 1027 / 1101 | 9277 / 10011 | 40.2 / 42.6 |
| q01 | 16 | 2068 / 2084 | 654 / 795 | 23831 / 24515 | 26.0 / 27.4 |
| q06 | 1 | 477 / 493 | 4069 / 4530 | 4144 / 4727 | 47.5 / 48.6 |
| q06 | 4 | 478 / 487 | 1154 / 1203 | 1174 / 1263 | 15.1 / 16.0 |
| q06 | 8 | 477 / 484 | 645 / 713 | 636 / 677 | 9.1 / 9.9 |
| q06 | 16 | 479 / 485 | 389 / 500 | 374 / 394 | 6.1 / 7.1 |

- Against Serial at the same workers, Exact is 1.10x, 2.01x and 3.16x faster on q01 at 4, 8 and 16
  workers. On q06 it is slower until 16 workers (1.23x faster there).
- RowOrder is slower than Serial on q01 at every count. On q06 it is 1.28x faster at 16 workers.
- On q06 at 16 workers, Exact (366 to 500 ms) and RowOrder (356 to 394 ms) overlap: no measurable
  difference between the arms there.
- Serial is one thread at every count, by design: a statement that sums a REAL goes back to the
  serial loop.
- Build: real_sums_ab.sh's release build (thin LTO, one codegen unit, overflow checks on). DuckDB
  runs `SET threads = n`. One binary for the three arms, so one calibration (RULES 5e).
- The answers above come from the same binary and SQL at each worker count, so the timed work is
  checked work. The limiters are not profiled.

## What each arm gives up

- **Serial (today).** Gives up speed. Any statement that sums a REAL runs on one thread: q01 takes
  2.07 s and q06 0.48 s at every worker count, about 80x DuckDB's time at 16 threads. Its answers
  equal SQLite's bit for bit on every case measured.
- **RowOrder (a).** Keeps Serial's answers bit for bit, at every worker count, on every case
  measured. It gives up speed and memory. It holds every SUM, AVG and TOTAL input with its row
  number until the end, then adds them up on one thread. Where many rows reach the sum, it is
  slower than Serial and gets slower with more workers (q01: 23.8 s at 16 workers). Its memory
  grows with the rows summed: one (position, value) pair per input, by the code. The peak was not
  measured.
- **Exact (b).** The only arm that speeds up with workers (q01: 3.2x Serial at 16). It gives up
  SQLite's answer wherever KBN rounds differently from the exact sum. q09 is built to show this
  (exact 1.0, SQLite 0.0), but the engine has not run it yet. Today it also answers NULL on
  overflow where SQLite says `inf`, a bug to fix before it could ship.
- **Both arms.** Neither matches PostgreSQL 19, which adds plainly in its own row order. A Postgres
  dialect needs its own `float8pl`/`float8_accum` aggregate whichever arm wins (R1).

## R1's side finding, in isolation (r1503, r1521 on adacachy2)

Test `r2_parallel_limit_finds_a_one_row_tables_row`, branch `perf/reals-r2-limit-repro` (c6788f95,
not for main). One-row table `bench_loaded`, in memory and on a file reopened by a fresh database.

- Parallel mode off: `SELECT 1 FROM bench_loaded LIMIT 1` returns its row; `count(*)` is 1.
- Parallel mode on, parallel LIMIT off, 4 cores: the LIMIT query returns its row (the serial loop
  answers it). `SELECT 1 FROM bench_loaded` with no LIMIT returns no row and no error.
- Parallel mode on and parallel LIMIT on, at 1, 2, 4, 8, 16 and 32 cores: no row, no error.

So the parallel scan loses the row, with or without LIMIT. Only `SELECT 1` over a one-row table was
tried. Not fixed (out of this lane's brief).

## Successor: commands

After r1468 ends, never beside it (both open the same database). On Mac Studio 2 (d1323898),
needs `macos/arm64,cpus=16,mem=12g`, sync from a clone of `perf/reals` (sync only after r1468
ends: it runs bench/real_sums_ab.sh from the same folder):

```
bench/real_sums_r2.sh 1 $HOME/.cache/amphora-perf/reals-out/sf1 fe02d165 answers-kbn answers-b limit
python3 -I bench/real_sums_answers.py ~/.cache/amphora-perf/reals-out/sf1   # q09 row, answers-b at 1/4/8/16
python3 -I bench/real_sums_stats.py ~/.cache/amphora-perf/reals-out/sf1     # adds timed-b
```

`ref-r2.json` (q09, TPC-H q01 and q06 references) is already in that folder, from r1515.
