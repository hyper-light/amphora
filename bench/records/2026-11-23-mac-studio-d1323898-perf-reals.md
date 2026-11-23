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
  - The twelve other REAL-summing TPC-H queries' answers at 1, 4 and 8 workers. r1468 answers them
    at 16 only. r1544 runs them (and q09, and the LIMIT check on the real database); it was still
    running when this was written. Their timings at every worker count are below (r1468 `timed-b`).
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
| TPC-H q03 q05 q07 q08 q09 q10 q11 q14 q15 q17 q19 q22 | all three | yes at 16; 1, 4 and 8 in r1544 | yes at 16, all 12 (SQLite 3.53.2) | no reference |

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

## Timings, the twelve other REAL-summing TPC-H queries (r1468 `timed-b`, 20:21Z to 20:25Z)

One round, 3 runs per cell, 0 failed. With 3 runs, p99 is the slowest run, and a gap under the
run-to-run spread (q03 and q05 at 1 to 4 workers spread by 0.5 to 0.9 s) is no measurable
difference. Load 5.8 to 11 on 32 cores. p50 / p99 ms.

| query | workers | Serial | Exact | RowOrder | DuckDB |
|---|---|---|---|---|---|
| q03 | 1 | 789.2 / 1533.8 | 781.4 / 1491.6 | 809.5 / 1583.0 | 83.6 / 107.8 |
| q03 | 4 | 1250.7 / 1288.9 | 1131.6 / 1145.9 | 1154.1 / 1186.6 | 32.7 / 33.4 |
| q03 | 8 | 1215.4 / 1236.5 | 1109.1 / 1123.7 | 1148.1 / 1179.5 | 23.2 / 23.7 |
| q03 | 16 | 1217.5 / 1224.6 | 1134.7 / 1138.4 | 1157.6 / 1169.7 | 18.9 / 20.7 |
| q05 | 1 | 1415.9 / 1703.3 | 1377.2 / 1652.6 | 1452.3 / 1760.1 | 108.2 / 121.5 |
| q05 | 4 | 1365.7 / 1374.3 | 1003.6 / 1379.7 | 1363.3 / 1655.8 | 53.4 / 54.4 |
| q05 | 8 | 1363.5 / 1374.8 | 1006.8 / 1362.6 | 1000.5 / 1351.6 | 44.1 / 44.6 |
| q05 | 16 | 1420.3 / 1652.0 | 1041.1 / 1411.4 | 1007.0 / 1347.4 | 39.4 / 39.8 |
| q07 | 1 | 1147.4 / 1160.1 | 1149.8 / 1176.3 | 1096.8 / 1171.4 | 110.1 / 110.4 |
| q07 | 4 | 1111.8 / 1122.2 | 1140.3 / 1146.4 | 1083.8 / 1094.9 | 55.2 / 55.3 |
| q07 | 8 | 1124.3 / 1128.1 | 1113.8 / 1120.4 | 1081.5 / 1088.5 | 44.7 / 44.7 |
| q07 | 16 | 1112.6 / 1119.5 | 1102.1 / 1108.1 | 1112.0 / 1118.4 | 40.3 / 41.1 |
| q08 | 1 | 169.9 / 183.3 | 169.8 / 184.0 | 158.0 / 174.6 | 110.5 / 126.2 |
| q08 | 4 | 162.7 / 163.3 | 167.6 / 168.0 | 160.3 / 162.5 | 62.8 / 63.4 |
| q08 | 8 | 163.2 / 164.0 | 162.3 / 166.0 | 165.1 / 165.4 | 55.6 / 55.8 |
| q08 | 16 | 161.8 / 163.7 | 162.6 / 166.1 | 163.7 / 166.9 | 52.1 / 52.3 |
| q09 | 1 | 1646.1 / 1693.5 | 1629.1 / 1658.0 | 1602.0 / 1609.3 | 258.5 / 271.0 |
| q09 | 4 | 1580.6 / 1588.2 | 1606.4 / 1613.7 | 1551.9 / 1565.1 | 82.8 / 83.1 |
| q09 | 8 | 1578.7 / 1596.5 | 1609.4 / 1632.7 | 1559.5 / 1571.0 | 65.6 / 66.4 |
| q09 | 16 | 1576.8 / 1587.4 | 1605.7 / 1607.9 | 1554.7 / 1578.8 | 53.8 / 54.0 |
| q10 | 1 | 562.3 / 575.0 | 563.4 / 563.4 | 545.4 / 563.7 | 145.9 / 158.9 |
| q10 | 4 | 548.3 / 551.8 | 551.6 / 575.8 | 547.6 / 551.3 | 56.9 / 57.1 |
| q10 | 8 | 549.6 / 552.5 | 552.1 / 559.7 | 539.1 / 554.9 | 45.9 / 47.2 |
| q10 | 16 | 542.7 / 554.5 | 559.9 / 568.6 | 539.1 / 543.0 | 41.5 / 41.5 |
| q11 | 1 | 68.2 / 93.5 | 73.8 / 100.2 | 65.0 / 91.6 | 92.5 / 94.6 |
| q11 | 4 | 69.8 / 70.0 | 75.0 / 79.2 | 64.1 / 67.6 | 21.4 / 21.7 |
| q11 | 8 | 65.0 / 71.2 | 81.4 / 81.9 | 64.6 / 67.5 | 17.7 / 17.7 |
| q11 | 16 | 66.2 / 70.1 | 75.1 / 80.6 | 66.4 / 67.5 | 17.8 / 18.1 |
| q14 | 1 | 470.9 / 473.5 | 491.9 / 494.3 | 468.0 / 468.8 | 57.2 / 57.6 |
| q14 | 4 | 466.9 / 470.2 | 490.7 / 491.7 | 468.3 / 472.6 | 25.7 / 25.9 |
| q14 | 8 | 469.3 / 470.8 | 489.2 / 490.4 | 466.1 / 472.4 | 20.6 / 21.0 |
| q14 | 16 | 468.8 / 473.4 | 494.7 / 496.2 | 465.8 / 467.7 | 18.0 / 18.1 |
| q15 | 1 | 900.8 / 904.4 | 956.6 / 963.8 | 892.8 / 893.2 | 444.3 / 454.4 |
| q15 | 4 | 908.4 / 908.5 | 959.9 / 974.3 | 889.0 / 890.0 | 45.9 / 46.3 |
| q15 | 8 | 900.3 / 902.4 | 960.3 / 973.4 | 891.0 / 904.3 | 28.6 / 29.0 |
| q15 | 16 | 905.2 / 905.4 | 972.5 / 1003.5 | 888.2 / 892.3 | 19.9 / 20.3 |
| q17 | 1 | 83.8 / 84.5 | 92.8 / 93.6 | 83.8 / 84.1 | 93.1 / 94.6 |
| q17 | 4 | 83.7 / 84.2 | 93.0 / 93.5 | 84.0 / 84.3 | 34.3 / 34.8 |
| q17 | 8 | 83.6 / 83.9 | 91.5 / 92.6 | 84.6 / 84.7 | 24.5 / 24.6 |
| q17 | 16 | 83.2 / 84.0 | 95.7 / 96.2 | 86.0 / 86.1 | 20.8 / 20.9 |
| q19 | 1 | 35.4 / 38.4 | 34.3 / 38.9 | 34.2 / 37.4 | 75.0 / 79.2 |
| q19 | 4 | 34.4 / 36.9 | 35.1 / 38.2 | 35.8 / 36.2 | 34.4 / 34.5 |
| q19 | 8 | 34.8 / 37.3 | 34.3 / 37.6 | 35.0 / 36.4 | 26.6 / 26.7 |
| q19 | 16 | 33.8 / 36.7 | 36.0 / 39.8 | 34.1 / 36.8 | 23.0 / 23.2 |
| q22 | 1 | 67.3 / 76.9 | 70.9 / 73.0 | 65.3 / 74.3 | 80.4 / 80.5 |
| q22 | 4 | 66.3 / 66.3 | 70.7 / 71.3 | 66.6 / 67.0 | 55.2 / 56.3 |
| q22 | 8 | 65.5 / 66.9 | 71.2 / 71.9 | 65.1 / 65.5 | 54.4 / 54.5 |
| q22 | 16 | 64.9 / 66.1 | 74.0 / 74.5 | 65.2 / 65.5 | 53.6 / 53.8 |

- With 3 runs per cell, most gaps between arms sit inside the run-to-run spread. The clear ones (no
  overlap between ranges) at 16 workers: Exact is slower than Serial on q11 (13%), q14 (6%), q15
  (7%), q17 (15%) and q22 (14%).
- DuckDB is faster than every arm at 16 workers on all twelve: 1.2x (q22) to 64x (q03).

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

# Lane R3 (reals), 2026-11-23 21:02Z to 21:55Z: r1544's answers in (rerun as r1562); the one-row loss traced to its cause, not fixed; Exact's overflow not fixed

## Verdict, losses first

- **Not done: one-row tables still lose their row.** The cause is traced (below), not fixed. R2's
  test fails on main; it is on branch `perf/reals-r3` (5d6fee36), not merged.
- **Not done: Exact still answers NULL on overflow.** The mechanism is read from the code, not run
  or fixed.
- An API error stopped the lane from about 21:30Z to 21:50Z.
- **Done: r1544's answers.** r1544 had failed at once: it synced the kiln checkout, not the Amphora
  clone (`bench/real_sums_r2.sh: No such file or directory`). r1562 reran R2's command.

## One-row tables lose their row in parallel mode: the cause (r1585, Mac Studio 2)

R2's test with print probes, in memory and on a reopened file, parallel mode on, at 1 to 32 cores.

- EXPLAIN of `SELECT 1 FROM bench_loaded` puts the loop body's copy at 5 (`start`, a Noop) and 6
  (`skip`, a Noop), and `keep` at 14. Address 14 is `Integer 1 -> r[5]`, the result column's
  constant, hoisted to the constant section after `Transaction`.
- A worker runs the body from 5, meets 6 (`skip`) before 14 (`keep`), and drops the row. Probes:
  `row 1 [Integer(1)] passed Ok(false) out []`, `rows run: leaves 0..1 ran 1 found []`, hook
  `answered Ok(([], true))`, `MorselNext: None`. The leaf walk (`leaves 1`), the scan (`ran 1`)
  and the dispatcher are right.
- With LIMIT the same happens (`keep` 19, the hoisted constant; `skip` 8).
- So the row is lost in `translate/parallel.rs` `emit_body`: `keep` is placed by
  `preassign_label_to_next_insn` after the outputs' code, and a constant output's instruction is
  hoisted with the label. Nothing here is particular to one-row tables. A guess from the mechanism,
  not run: any statement whose result columns are all constants loses every row this way.
- Next: bind `keep` to the body's own Noop after the outputs, then run R2's test (1 to 32 cores,
  with and without LIMIT, memory and file) on main.

## Exact answers NULL on overflow: the mechanism (from the code, not run)

- `ExactSum::add` (`vdbe/morsel.rs`): when `x + y` overflows, the two-sum's error term
  `low = y - (high - x)` is -inf or NaN, and `low != 0.0` stores it as a partial. Later adds and
  the workers' merges meet it; inf plus -inf is NaN, and `is_nan` makes the answer NULL.
- Dropping the poisoned partials is not enough. In bigpairs (+1e308 +1e308 -1e308 -1e308 within
  each order) the running sum overflows up, then down. The exact sum is +inf only because orders
  with fewer than four lines leave unmatched +1e308s (R1's big-integer reference). Taking "the
  running sum overflowed" as the answer still gives NaN there. The sum has to stay exact past the
  double's range.

## r1544's answers (rerun as r1562, Mac Studio 2, 21:08Z to 21:12Z)

Binary fe02d165 (r1468's), from perf/reals 294efb6a. Load 3.1 to 5.3 on 32 cores.
`bench/real_sums_r2.sh 1 ~/.cache/amphora-perf/reals-out/sf1 fe02d165 answers-kbn answers-b limit`,
then `python3 -I bench/real_sums_answers.py ~/.cache/amphora-perf/reals-out/sf1`.

### KBN's loss case, q09

| column | Serial (1 worker) | Exact at 1/4/8/16 | RowOrder at 1/4/8/16 | SQLite 3.53.2 | SQLite ref (func.c) | PostgreSQL 19 ref | exact ref |
|---|---|---|---|---|---|---|---|
| sum(kbnloss) | 0.0 | 1.0 at all | = Serial at all | 0.0 | 0.0 | -1e+18 | 1.0 |
| avg(kbnloss) | 0.0 | 1.6663292349965798e-07 at all | = Serial at all | 0.0 | 0.0 | -166632923499.658 | 1.6663292349965798e-07 |
| total(kbnloss) | 0.0 | 1.0 at all | = Serial at all | 0.0 | 0.0 | n/a | 1.0 |

As the code says: Serial and RowOrder give SQLite's answer, 0.0. Exact gives the exact sum, 1.0.
This is the one measured case where Exact and SQLite differ apart from Exact's overflow bug.

### The twelve other REAL-summing TPC-H queries at 1, 4 and 8 workers

q03 q05 q07 q08 q09 q10 q11 q14 q15 q17 q19 q22: every arm at 1, 4 and 8 workers (and at 16, from
r1468) is bit-identical to Serial at 1 worker and to SQLite 3.53.2. No cell differs. These have no
func.c, PostgreSQL or exact reference.

### R2's limit phase did not run

It exited 1 at every core count before opening the database: the bench wants `q02.sql` in the
folder it is given (`limit-queries/q02.sql: No such file or directory`, in `limit.err`). It says
nothing about the engine.

# Lane R4 (reals), 2026-11-23 21:57Z to 22:52Z: parallel mode keeps every row; Exact stays exact past the double's range

## Verdict, losses first

- **The module suites after the fix did not finish in this lane.** r1639 ran `vdbe::morsel` and
  `translate::parallel` for 17 min without ending and was stopped so clippy could run. Rule 5b puts
  them after the merge: the successor's first run.
- **No timings.** Parallel mode adds one Noop per row to each worker's body run (a fence `keep` is
  placed after). Its cost is not measured.
- **Done: the lost-row bug is fixed** (9a83c1eb). R2's test and the new constant-columns test pass.
- **Done: Exact stays exact past the double's range** (199205b2). bigpairs answers +inf, as SQLite
  does; a running sum that overflows and comes back answers the exact sum.
- The default stays `Serial`; nothing here changes it.

## The lost row (r1639, Mac Studio 2, load 1.7 to 3.5 on 32 cores)

R3's trace held. `emit_body` placed `keep` with `preassign_label_to_next_insn` right after the
outputs' code. A label resolves to the instruction after the one emitted before it, wherever that
one moves (`resolve_labels`: anchor + 1 after `emit_constant_insns` remaps it). A constant output
is hoisted to the program's end, so `keep` followed it there and every worker reached `skip` first.
Fix: a Noop of the body's own before `keep`'s, so `keep`'s anchor is never hoisted.

It was wider than R3 guessed: any statement whose **last** result column is a constant lost every
row, not only all-constant ones.

`r4_parallel_constant_columns_return_every_row`: 20,000 rows; `SELECT 1`, `SELECT 1, 'two', 3.5,
NULL` and `SELECT a, 7`; parallel mode at 1, 2, 4, 8, 16 and 32 cores; in memory and on a file
reopened by a fresh database; against the serial loop's rows.

| commit | R2's one-row test | constant columns |
|---|---|---|
| 5dcb420a (test, no fix) | fails | fails: 0 rows for every statement, core count and storage |
| 9a83c1eb (fix) | passes | passes |

## Exact past the range (r1655, Mac Studio 2, load 1.5 to 2.6)

`ExactSum` is now its partials plus a carry of 2^1021 units (an i64). Whole units move into the
carry, so every partial stays below 2^1021 and no two-sum step reaches 2^1023. `value()` rounds
once: with no carry as before; with 1 or 2 units the carry is the largest partial; with 3 to 9 at
2^-1021 the scale (a partial that would vanish there keeps its sign, since it can only settle a tie),
then scaled back; past 9 an infinity. The non-finite inputs are added after. R3's reason holds:
dropping the infinite error terms alone would still answer NaN on bigpairs.

| commit | `r4_exact_sum_stays_exact_past_the_range` | G8 hard cases (11, at every core count) |
|---|---|---|
| d25b53fd (tests, no fix) | fails: bigpairs, cut at 0, NaN | fails: bigpairs `[[Null, Null, Null]]`, expected `[[inf, inf, inf]]` |
| f7162e53 (fix) | passes | passes |

The unit test's cases, each cut in two at every place, merged both ways and through the payload's
bytes: bigpairs (+inf), overflow and back (1.5), down and back (-0.25), max + max - max (max),
max + 2^970 (the tie: +inf), max + 2^970 - 2^-1074 (the tiny term breaks the tie: max).

## Side findings (not chased, rule 0)

- `storage/versioned/columns/scan.rs` keeps its own `ExactSum` and moves an overflowed partial sum
  into `special` the old way. Read from the code, not run: the column copy under `Exact` likely
  answers NULL on bigpairs too.
- Any label placed by `preassign_label_to_next_insn` right after an instruction that gets hoisted
  follows it to the constant section. Only `keep` was found doing this in `parallel.rs`; the rest of
  the translator was not searched.
