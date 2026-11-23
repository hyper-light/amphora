# Columnar lane G6 on adacachy2: why parallel doesn't scale, vectorized scans, q01's row cost (2026-11-23)

Ada's goal: Amphora about 10x faster than DuckDB on analytics, with answers identical to the serial
engine. Lane M2 (2026-11-23-adacachy2-perf-analytics.md) measured the opposite: 9.43x slower than
DuckDB on 32 threads, and Amphora's parallel mode 0.87x as fast at 8 cores as at 1. This lane asked
why parallel gets slower with cores, whether vectorized scans should be on, and where q01's 315 ns a
row goes.

## Verdict, losses first

- **Nothing to land for scaling this hour.** Amphora's parallel mode never started a worker on q01
  or q09 at 8 cores (measured below), and q06 declines too. From 1 to 8 cores DuckDB got 10.2x
  (q01), 14.1x (q06) and 6.8x (q09) faster; Amphora stayed on one thread. The morsel path declines these statements before any worker
  starts, so the 1-to-8-core "slowdown" in M2's record is the serial loop run twice on a shared
  machine, not parallel overhead. The fix at the cause is parallel SUM over REAL with the serial
  answer, which waits on Ada's open choice (row-order KBN against an exact sum everywhere). The
  engine scales when let in: q01 with its sums made INTEGER runs 6.7x faster on 8 cores than on 1,
  with the same answer.
- **Vectorized scans are slower: 2.0x on q01 and 2.3x on q06** (medians of 20 interleaved runs a side, ranges apart, same answers). The default stays off; the branch that flips it (perf/columnar-g6-vec) is not for main.
- **q01's row cost, no fix landed:** over a quarter of q01's time is reading columns out of records
  one value at a time (`op_column` 27%), then the interpreter's dispatch (11 to 14%), the GROUP BY
  lookup (10 to 14%) and the aggregate steps (11%). The existing fix for the largest part, vectorized
  scans, makes it slower (above), so its two causes are this lane's handover, not a merge.

## Unit 1: workers started, morsels, waits

Method. adacachy2 has no `perf`, and `perf_event_paranoid` is 2, so samply refuses to record
("needs to be set to 1 or lower"). ptrace scope 1 still lets a tracer that starts the process trace
it, so the profile here is a gdb sampler (`pmp.py`, `pmp.sh` in this record's folder): gdb runs the bench, a shell
loop sends SIGINT every 50 ms, and each stop records every thread's stack. A second, untraced run of
the same command samples `/proc/<pid>/task/*/{stat,status}` every 20 ms (`tasks.py`): every thread the
process started, its CPU time, and its voluntary context switches (a thread waiting on a lock, a
condition or I/O switches voluntarily). Binary: lane M2's shipping build, amphora-bench at 29eb13ba
(main 364266aa + bench fixes), `--planner --bench tpch --sf 1 --targets amphora --cores N --runs 2`,
on a reflinked copy of M2's SF1 database.

| query | cores asked | gdb samples | threads in every sample | busy frames | threads /proc saw | CPU, 2 runs | voluntary switches |
|---|---:|---:|---:|---|---:|---:|---:|
| q01 | 1 | 123 | 1 | `dispatch_loop<false>` (serial VDBE) | 1 | 5430 ms | 30 |
| q01 | 8 | 127 | 1 | `dispatch_loop<false>` | 1 | 4830 ms | 38 |
| q09 | 8 | 365 | 1 | `dispatch_loop<false>` | 1 | 13200 ms | 23 |

So at 8 cores: **workers started 0** (one thread for the whole process), **morsels handed out 0** (no
`op_morsel_aggregate` frame in any sample), **time per worker** not applicable, and **waits** none to
speak of (23 to 38 voluntary switches in 5 to 13 s of CPU). There is no contention, serial merge or
lock to fix in these runs: the parallel path is not taken.

Why it is not taken. The morsel hook (`translate/parallel.rs`) refuses a loop body that reads any
column of REAL affinity: `body_reads` (parallel.rs:440, run over the filter and the outputs at 521 to 523, and over a join's terms at 1502 to 1505) takes only columns "stored as
written", of INTEGER, TEXT, BLOB or no affinity, because the pipeline compares GROUP BY values, MIN and
MAX bytewise. On top of that, `no_real` sends a statement to its serial loop at run time when a SUM
output or GROUP BY key holds a REAL, because a parallel REAL sum adds in another order than the serial
loop's row-order KBN sum and can differ in the last bits. The bench's Lite dialect stores TPC-H's
decimals as REAL (`for_dialect`: `{DEC}` is `REAL`), so q01 (sums over l_quantity, l_extendedprice,
l_discount, l_tax), q06 (filters on l_discount and l_quantity, a sum of l_extendedprice * l_discount)
and q09 (a sum of amounts) are all refused at plan time, before any worker could start.

The probe that tests it (`threads2.sh`, r715): the same binary and database, with q01 and q06 rewritten
to sum an INTEGER column (`sum(l_linenumber)`, `count(*)`) and everything else kept: q01's scan, its
TEXT date filter, its GROUP BY and ORDER BY; q06's REAL filter terms.

| query | cores | threads /proc saw (3 runs) | worker CPU each run | median of 3 | first row |
|---|---:|---:|---:|---:|---|
| q01 with an INTEGER sum | 1 | 1 | - | 5142 ms | A, F, 4439683, 1478493 |
| q01 with an INTEGER sum | 8 | **22** (the caller + 7 workers a run) | 640 to 660 ms over 680 ms | 648 ms | A, F, 4439683, 1478493 |
| q06 with an INTEGER sum | 8 | 1 | - | 790 ms | 76033, 228214 |
| q06 as TPC-H has it | 8 | 1 | - | 701 ms (one of 3) | 123141078.2283 |

So the parallel engine itself starts 7 workers, keeps them busy (2 to 4 voluntary switches each) and
gives the same first row at 1 and 8 cores, as soon as q01's sums are integers: q01's gate is its REAL
sums. q06 stays on one thread even with an integer sum: its filter still reads the REAL columns
l_discount and l_quantity, which `body_reads` refuses. So lifting `no_real` alone (the SUM decision)
would not parallelize q06; REAL columns that are only filtered or computed on would have to be let
into the copied loop body too (it runs the serial loop's own RealAffinity, so their values are the
serial loop's). These timings ran at load 700 to
1160 on 32 cores and say nothing about speed; the thread counts are what this probe measures.

The ladder (r751): M2's binary, `--targets amphora,duckdb-mt --only q01,q06,q09 --cores 1,2,4,8
--runs 5`, then the INTEGER-sum q01 alone on the same ladder, medians of 5 (12 cores held; kiln could
not give 16 idle cores this hour, so the 16-core point is missing).

| query | engine | 1 core | 2 | 4 | 8 | 1 to 8 |
|---|---|---:|---:|---:|---:|---:|
| q01 | amphora | 3075 ms | 3379 | 3726 | 1904 | one thread throughout |
| q01 | duckdb-mt | 498 | 112 | 61 | 49 | 10.2x |
| q06 | amphora | 567 | 794 | 493 | 475 | one thread throughout |
| q06 | duckdb-mt | 184 | 38 | 20 | 13 | 14.1x |
| q09 | amphora | 5083 (1 run, then a 30 s timeout) | 2179 | 1234 | 1102 | one thread throughout |
| q09 | duckdb-mt | 568 | 507 | 123 | 84 | 6.8x |
| q01, INTEGER sums | amphora | 3251 | 1686 | 818 | 487 | **6.7x** |

- Answers: at every core count Amphora's first row is the same as at 1 core and the same as
  DuckDB's (q01 sum_qty 37734107, q06 123141078.2283, q09 ALGERIA 1998 27136900.1803). Every run
  finished except one q09 run at 1 core, which timed out at 30 s.
- Amphora's TPC-H times move with the machine, not the cores: the ladder runs 1 core first and 8
  last, and the load fell from 306 to 56 meanwhile. The probe above shows one thread at 8 cores.
- q09's 30 s timeout at 1 core, beside 1.1 to 5.1 s runs of the same serial code, is the planner
  choosing a different plan on some runs (lane T2's finding that plans vary with learned weights),
  the same as M2's q09 timeout at 8 cores. It is not parallelism.
- Once its sums are integers, the q01 shape scales 1.93x, 3.97x and 6.68x at 2, 4 and 8 cores, with
  the same first row at each. So the morsel engine scales when it is let in; the gate is the limiter.

The fix at the cause is a parallel REAL SUM whose answer equals the serial one. Lanes G1 to G3 built
`ExactSum` (Shewchuk's exact partials) for that, and Ada's call of 02:50Z is to A/B both designs
(row-order KBN against an exact sum everywhere) and decide on the data. That decision is still open,
so this lane changed nothing on the parallel path.

What the "0.87x at 8 cores" in M2's record was. With no worker started, the 1-core and 8-core points
ran the same serial code. M2's 8-core point (r603) ran beside r492 on the same machine at load up to
54. This lane's own single runs swing just as much under load (q01 2606 ms at load 56 to 67 against
M2's 1840 ms at load 18). So that ratio measured the machine, not parallelism.

## Unit 2: vectorized scans (QT.7) default-off

Why they are off, from the code and the record: `Connection::set_vectorized_scans` says "Off until
its measurement against the row-at-a-time scan (the coordinator's) says otherwise", and
docs/design/executor.md decision 9 names the test that would falsify it: AC-QT.7.1, q06 at SF 1 not
faster per row with the switch on than off, run as `amphora-bench --planner --targets
amphora,amphora-vectorized`. STATUS.md lists AC-QT.7.1 to .3 as "measured (the coordinator's)", still
open. No failing test or wrong answer is recorded against the switch.

The A/B (`vecab.sh`, r694): M2's binary, `--targets` in both orders, alternating (amphora first,
then amphora-vectorized first, twice), 5 timed runs per target per invocation after each target's
warming run, q01 and q06, on reflinked copies of one SF1 database.

| query | amphora (switch off) median, range | amphora-vectorized (on) median, range | on over off | runs per side | first row equal |
|---|---:|---:|---:|---:|---|
| q01 | 2643 ms (2115 to 3153) | 5273 ms (3863 to 7830) | 2.00x slower | 20 | yes |
| q06 | 624 ms (482 to 924) | 1439 ms (1170 to 2193) | 2.31x slower | 20 | yes |

**Vectorized scans are slower, not faster: 2.0x on q01 and 2.3x on q06.** The ranges don't
overlap, and both arms ran interleaved under the same load (81 to 1189 on 32 cores, 13:26Z to 13:30Z), so the
direction holds even though the absolute times are inflated. By decision 9's own test (AC-QT.7.1,
q06 not faster per row with the switch on) the design as built is falsified on this machine. So the
default stays off: flipping it would make q01 and q06 about twice as slow.

Why it is slower. One gdb profile of q06 with the switch on (r738, 86 busy samples, one thread):
`op_column` 62% of the time, of which serving columns from the batch (`serve_column`) 58% and
decoding batch windows (`decode_window`) 47%; stepping the cursor 26%. Two causes in
`vdbe/leaf_batch.rs`, read from the code (inferred, not separately timed):

1. A window decodes every column the statement reads for every row in it. The row-at-a-time loop
   reads `l_shipdate` first and never reads q06's other three columns for the rows its date test
   rejects (q06 keeps one year of about seven, so most rows). The batch decodes all four for every
   row: no late materialization.
2. Each value is decoded once into the batch, then cloned into the register on every `Column`
   (`serve_column`: `value.try_clone()`, an allocation for each TEXT value such as `l_shipdate`,
   `l_returnflag` and `l_linestatus`). The row-at-a-time loop decodes straight into the register,
   reusing its buffer.

Both queries still ran through the per-row `Column` opcodes in `dispatch_loop`; the fused
whole-batch loop of decision 10 was not what ran. A batched scan that wins would decode a column only
for the rows still selected and hand values to the consumer without a copy each.

Correctness with the switch on by default (branch perf/columnar-g6-vec eade4a9a, which flips
`vectorized_scans` in `database.rs` to true), against main 11ed634e
with it off, in one run, one after the other (r747): `cargo test -p amphora-engine --lib -- t_qt_6_
cb4_ leaf_batch:: t_qt_7 --skip t_qt_6_18 --test-threads=8` (t_qt_6_18 skipped: it hangs on main,
lane G2's finding, and did again here past 60 s).

| arm | passed | failed | failing only with the switch on |
|---|---:|---:|---|
| main, switch off | 36 | 9 | - |
| switch on by default | 37 | 8 | none |

Both arms fail the same 8: cb4_ab_both_rebuy_policies_at_the_extremes,
cb4_a_read_only_transaction_reads_the_copy_at_its_snapshot, and t_qt_6_24, 6_29, 6_31, 6_32, 6_33
and 6_35. Those are main's own (lane G5's fixes for 6_24, 6_31 and 6_32 are with integrator J2).
t_qt_6_30 failed on main and passed with the switch on here, but it also failed with the switch on
in an earlier run of the same branch (r688), so it is flaky, not fixed by the switch. So the switch
breaks no test in these suites; it is held back by speed alone.

## Unit 3: where q01's time goes

250 gdb samples of q01 on one thread (the 1-core and 8-core runs above, the same serial code), as a
share of busy samples (each frame counted once per sample; children inside their callers):

| where | share | what it is |
|---|---:|---|
| `op_column` / `op_column_fetch` | 27% | reading a column out of the row's record: locate the payload, parse the serial type, decode into a register (`decode_serial_type_into_register` alone 5 to 6%) |
| `op_next` / `next_row` | 14 to 16% | stepping the cursor, page reads included (`read_page` 12% in the 8-core run, `pread64` about 5%: the first of the two runs in a fresh process reads every page from the OS) |
| `op_group_find` | 10 to 14% | the hash GROUP BY's lookup of (l_returnflag, l_linestatus) per row |
| `op_agg_step` | 11% (8-core run; under 13% in the 1-core run) | the 8 aggregates, KBN sums (`kbn_step_finite`, `agg_step_fast`) |
| `dispatch_loop` self | 11 to 14% | the interpreter's own dispatch per instruction |
| `op_real_affinity` | 3 to 4% | REAL affinity applied per column read |
| `op_multiply` and `op_subtract` | 3 to 5% | `l_extendedprice * (1 - l_discount) * (1 + l_tax)` |

The largest cause is decoding values out of records one column of one row at a time, an interpreter
dispatch each: q01 reads 7 columns of each of 5.9 M rows. That is what QT.7's vectorized scans
replace (each leaf decoded once into column batches, and a fused loop for filters and aggregates).
Interpretation per row (dispatch, group find, aggregate steps) is the next third; value boxing did
not show as a separate cost (the unnamed frames under `alloc` are 3 to 6% of samples).

Statistical error: with 250 samples a 27% share is plus or minus 3 points (one standard error).

## Handover: what would move these numbers, in order

1. Run Ada's SUM-over-REAL A/B (row-order KBN against an exact sum everywhere, her call of 02:50Z) and
   land the winner. It is what keeps q01's and q09's sums off the workers.
2. Let REAL columns that a loop body only filters on or computes with into `body_reads`, keeping
   `no_real` for SUM outputs and GROUP BY keys (and MIN and MAX). The copied body runs the serial
   loop's own RealAffinity, so the values are the serial loop's; t_qt_6_* plus a REAL-filter
   differential would hold it. Without this, q06 stays serial even after step 1.
3. Make the bench record the workers each Amphora run started (`RunStats.workers` is returned to
   `op_morsel_aggregate` and dropped there), so a ladder can't again report cores it never used.
4. Vectorized scans: decode a column only for rows the filter still keeps, and hand values on
   without a clone each, then rerun `vecab.sh`; or retire QT.7 for compiled pipelines (G5 step 2), as
   decision 9 allows.
5. q09 timed out once at 1 core here and at 8 cores in M2's ladder, with the same serial code: a
   planner question for lanes T2 and T3, not this lane's.

## How far to trust it

- adacachy2 was shared and overloaded throughout: load 56 to 1189 on 32 cores (13:17Z to 13:35Z),
  CPU pressure some avg10 10 to 61%. kiln held only 1 to 10 cores for these runs. Every timing here is
  directional; the counts of threads and samples are not affected by load.
- The thread counts and stacks are counts, not times, and need no repetition to be trusted. Each
  query's 2 runs ran in one process, so each saw the same plan.
- gdb stops all threads per sample, which costs time but does not change which code runs.

## Files

`2026-11-23-adacachy2-perf-columnar/`: pmp.py and pmp.sh (the gdb sampler, the only profiler that
works on adacachy2 as it is set up), agg.py (stacks to inclusive, leaf and caller shares), tasks.py
(threads, CPU and context switches from the /proc samples), threads2.sh (the INTEGER-sum probe, with
qx/), vecab.sh (the interleaved vectorized A/B), summ.py (medians, ranges and first rows from the
bench's JSONL). Raw outputs stay on adacachy2 under ~/.cache/amphora-g6 (prof/, ab/, thr/, tests/).
