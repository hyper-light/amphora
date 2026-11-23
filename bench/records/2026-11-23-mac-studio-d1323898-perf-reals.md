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
