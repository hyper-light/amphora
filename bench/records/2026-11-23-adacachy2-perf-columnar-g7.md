# Columnar lane G7 on adacachy2: REAL columns into the parallel scan; the SUM-over-REAL A/B not yet built (2026-11-23)

Ada's goal: Amphora about 10x faster than DuckDB on analytics, with answers identical to the serial
engine. Lane G6 found that Amphora starts no workers on TPC-H q01, q06 and q09: the parallel hook
refuses REAL columns at plan time and REAL sums at run time, and TPC-H's decimals are REAL. Ada's
call (02:50Z) is to decide SUM over REAL from an A/B of (a) a row-order Kahan-Babuska-Neumaier sum
and (b) an exact sum everywhere. This lane was to build both, measure them, let REAL filters and
keys into the parallel path, and then look at the vectorized scan's late decode.

## Verdict, losses first

- **The SUM-over-REAL A/B was not built and has no numbers.** Each arm is several hundred lines
  whose point is bit-exact answers, and an hour (with builds refused for room) did not hold both
  plus tests that prove it. The handover below gives the design for each arm, checked against the
  code, so the next lane starts at the code.
- **Two engine tests are flaky in a way that follows the executable.** t_qt_6_30 and t_qt_6_29
  assert that a JOIN's hook runs at 1 stated core. Whether it runs changes with the binary even
  where the compiled programs are the same: an arm that differs from a passing arm in one constant
  that cannot reach those statements failed 10 of 10 against 0 of 10. These tests cannot judge an
  engine change until they stop depending on calibration (unit 2 below).
- **No TPC-H time moves yet.** Unit 2 merged, but it does not move q01 or q06: a SUM, AVG or TOTAL
  over REAL still sends the statement to the serial loop at run time, and both have one.
- **The real-data probe did not run.** The probe that would show q06's shape starting workers on
  the SF1 data (G6's `threads2.sh` with an INTEGER sum) needs a release bench build; kiln refused it
  for room twice (r829: 0 of 32 cores idle).
- **The bench workspace has no Cargo.lock.** An offline build of `bench/` resolves other dependency
  versions than the root lock, and main's `amphora` lib then fails to compile (5 errors on
  `hyper_rt` imports, r815). Seed it first: `cp Cargo.lock bench/Cargo.lock`.
- **Unit 3 (decode only the rows the filter keeps, without copies) was not started.**

## Unit 2: REAL columns in a one-table scan's workers

The change is lane G2's 94b94f41, never run until now, cherry-picked as 8f38f8b3 on
perf/columnar-g7 and merged to main with this record. A one-table scan's copied
loop body now takes columns of REAL and NUMERIC affinity in its filter, its outputs and its
aggregates' FILTERs (`body_reads_numeric`, translate/parallel.rs). A worker loads a REAL column's
stored INTEGER as a REAL, as the serial loop's `RealAffinity` after its `Column` does
(`MorselBody::reals`). Joins still refuse REAL columns at plan time.

Why the answers cannot depend on the worker count, from the code:

- Filters and computed values run the serial loop's own opcodes on the same values, row by row.
- GROUP BY keys compare by SQLite's numeric equality (`morsel::key_part`: an integral REAL is its
  INTEGER, so 0.0 and -0.0 are one group), and a merged group keeps the key values of its earliest
  row (`ScanAggregate::merge` keeps the lower `first_row`), so 1 against 1.0 or 0.0 against -0.0
  comes out as the serial loop's first row has it.
- MIN and MAX compare by `Value`'s order and keep the earliest row on a tie.
- A SUM, AVG or TOTAL output holding a REAL still sends the statement to the serial loop
  (`no_real`, which for a one-table body holds exactly those outputs), and `Accumulator::Mean`
  refuses a REAL.

Checks (adacachy2, `CARGO_INCREMENTAL=0`, lane target dir):

| run | head | what | result |
|---|---|---|---|
| r811 | 8f38f8b3 (the commit on main d728592f) | `cargo clippy -p amphora-engine --all-targets --locked -- -D warnings` | clean |
| r811 | 8f38f8b3 | `cargo test -p amphora-engine --lib -- audit_3_p2 t_qt_6_ morsel --skip t_qt_6_18 --skip t_pl_8_14` | 25 passed, 7 failed |
| r823, r834 | main d728592f against 8f38f8b3, alternated | t_qt_6_30 and t_qt_6_29 alone | the tables below |
| r847 | 4fb6d457 (merged with main b122f970), and main b122f970 | clippy, then the same tests on both | clippy clean; the same 7 failures on both |

The test that holds the change, `audit_3_p2_real_and_numeric_columns_run_in_parallel_as_the_serial_loop`,
passes: ten statements over a REAL and a NUMERIC column (filters, GROUP BY a REAL, GROUP BY
`typeof`, MIN and MAX, computed keys, rows out) answer as the serial loop at 1, 2, 4, 8 and 16
stated cores, the copied body runs over every row, and more than one worker runs it; SUM, AVG and
TOTAL over REAL answer the same from the serial loop.

The 7 failures in r811: t_qt_6_24, 6_29, 6_31, 6_32, 6_33 and 6_35 fail on main too (J2's main
baseline, LANES 13:16Z, and G6's r747). The seventh is the regression.

The A/B of the test binaries, built in one target dir from one base (main d728592f), run
alternately, one test at a time, load 41 to 65 on 32 cores. The inert arm (deb631a0, branch
perf/columnar-g7-null, not for main) compiles G2's code in but admits no REAL column and loads
none as REAL, so its programs are main's:

| test | main d728592f fails | G2's change 8f38f8b3 fails | inert arm deb631a0 fails |
|---|---:|---:|---:|
| t_qt_6_30 | 1 of 23 (r823 0/3, r834 0/10, r863 1/10) | 15 of 24 (r811 1/1, r823 1/3, r834 5/10, r863 8/10) | 0 of 10 (r863) |

Every t_qt_6_30 failure, on every arm, is the same JOIN statement at 1 stated core: `SELECT t.c,
avg(u.g), total(t.b + u.g) FROM t JOIN u ON t.a = u.k GROUP BY t.c on 1 cores: the hook didn't
run`. In the suite with 2 test threads, current main b122f970 failed it too (r847, one run), so
main has the flake; G2's change makes it the common case.

t_qt_6_29 fails on every arm, but where it fails moves. Main and the inert arm always fail at its
2nd join (`SELECT u.g, count(*), max(t.e) ... WHERE u.g = -3 GROUP BY u.g`): main 15 of 15
(r834 10, the 14:27Z run 5), the inert arm 5 of 5 (the 14:27Z run). G2's change fails at its 1st join (`SELECT t.d,
count(*), sum(u.g) FROM t JOIN u ON t.a = u.k GROUP BY t.d`), which main passes, in 12 of 15 (r834
10 of 10, the 14:27Z run 2 of 5), and at the 2nd in the rest. That test's tables hold no REAL or NUMERIC
column at all.

Then the change was split in two, each half alone (r873, 10 alternated rounds, one base):

| arm | what it keeps of G2's change | t_qt_6_30 fails | the statements that fail |
|---|---|---:|---|
| main d728592f | nothing | 0 of 10 | |
| 8f38f8b3 | both halves | 5 of 10 | the `GROUP BY t.c` join, 5 |
| plan-only 4c0790c6 | REAL admitted at plan time, none loaded as REAL | 10 of 10 | the `GROUP BY t.c` join 5, `SELECT avg(t.b), total(u.g), avg(u.g) FROM t JOIN u ON t.a = u.k WHERE u.g > 0` 5 |
| load-only 2ef87485 | REAL loaded as REAL, none admitted at plan time | 9 of 10 | the `WHERE u.g > 0` join 7, the `GROUP BY t.c` join 2 |
| inert deb631a0 (r863) | neither | 0 of 10 | |

What this says about the cause:

- **The failures follow the executable, not the change's logic.** The plan-only arm differs from
  the inert arm in one constant (whether `body_plan` admits REAL and NUMERIC columns). `body_plan`
  serves only one-table scans, and every statement in t_qt_6_30 up to the failing joins reads
  table t, which has no REAL or NUMERIC column. So the two arms compile the same programs for those
  statements, and still one fails 10 of 10 and the other 0 of 10. Each half alone fails more often
  than both together, and t_qt_6_29 moves the same way with no REAL column in its tables.
- What the executable changes is inferred, not traced: the hook decision at 1 stated core
  depends on the plan, and the planner's learned costs are calibrated once a process from timed
  work (translate/optimizer/learned_costs.rs). Lane T4 found today that this calibration differs
  per executable (t_pl_8_13). A code change anywhere in the engine can move these tests.
- So these assertions at 1 stated core are flaky on main as well (main d728592f failed t_qt_6_30
  in 1 of 43 runs alone, main b122f970 in its one suite run, r847) and do not judge this change.
  The fix that would make them judge it is to state the planner's learned costs in these tests, as
  `MachineCosts::stated` states the morsel costs, so the plan does not depend on calibration.

The merge gate (RULES 5b) on the merged head 4fb6d457 against main b122f970, in one run (r847):
clippy clean; G2's test passes; the 7 failures on the branch are exactly main's 7 in the same
run (t_qt_6_24, 6_29, 6_30, 6_31, 6_32, 6_33, 6_35). Unit 2 merged on that.

## Unit 1 handover: the two SUM designs, checked against the code

Where the code is today:

- Serial loop. SUM and TOTAL keep `[acc, r_err, approx, ovrfl, count]` and AVG keeps
  `[sum, r_err, count]`, as `Value`s (vdbe/execute.rs `init_agg_payload`, the `Sum | Total` and
  `Avg` arms of the aggregate step, `finalize_agg_payload`). SUM is SQLite's: an exact i64 sum until
  the first REAL, then KBN seeded by `kbn_init_from_int` (which splits a large integer so no bit is
  lost). The payload's width is fixed in the compiled program (`num_payload` in the insns).
- Workers on a one-table body. `TableBodyAggregate::run` folds each passing row into
  `ScanAggregate::fold`, one `Accumulator` per aggregate. `Accumulator::Sum` already keeps REALs
  in an `ExactSum` (Shewchuk partials); `Accumulator::Mean` (AVG, TOTAL) refuses a REAL; and
  `no_real` (parallel.rs, `no_real: sums` for one-table bodies) refuses REAL sum outputs per row.
- `morsel::execute` runs a pilot on the caller's thread first (leaves `0..done` in doubling
  chunks), then workers claim morsels from one atomic counter, each folding into its own partial,
  and the partials merge once at the end.

The switch: one connection setting read at run time (for example `RealSum::RowOrder` and
`RealSum::Exact`), and one bench target per arm. `RowOrder` keeps today's serial answer.

Arm (a), row-order KBN:

- The serial loop is unchanged, so its bits are today's.
- Workers keep each SUM, AVG and TOTAL input of a morsel per group in row order instead of folding
  it: one run per `run()` call, tagged with its first leaf. The pilot's chunks are runs too, so the
  runs tile `0..total`.
- A sequencer the workers share (a mutex in the pipeline) folds runs in leaf order. The thread
  that brings the next expected run folds it at once, then any held successors; a run that comes
  early waits in a map by its first leaf. The fold replays the serial step code (the same `Sum |
  Total` and `Avg` arms on the same payload `Value`s) and finishes with `finalize_agg_payload`, so
  the bits equal the serial loop's at every worker count by construction.
- Joins: G2 found that a probe row's matches come in hash-table order, not the serial loop's. A
  join's REAL sums therefore stay serial under (a) unless each probe row's matches are put in the
  serial loop's order, so q09 stays serial under (a).
- The limiter to measure: the sequencer folds every row of every REAL aggregate one thread at a
  time. q01 has 7 REAL aggregates (4 SUMs, 3 AVGs) over 5.9 M rows, about 41 M steps. At 2 to 8 ns a
  step (a guess, not measured) that is 80 to 330 ms, against DuckDB's 49 ms on 8 cores. Peak memory
  is the runs held while an earlier one is still running.

Arm (b), an exact sum everywhere:

- Workers: `Accumulator::Sum` is already exact. `Accumulator::Mean` needs an `ExactSum` for REALs,
  and `no_real` drops its sum outputs under `Exact`. Integer inputs stay in an i128 and join the
  `ExactSum` at the end, split as `kbn_init_from_int` splits them, so the sum stays exact.
- Serial loop: SUM, TOTAL and AVG over REAL accumulate exactly and round once in
  `finalize_agg_payload`. The payload is `Value`s of a width fixed at translate time, so either
  translate reads the switch and widens the payload, or the partials live in one BLOB slot.
- Joins: an exact sum does not depend on order, so a join body could take REAL sums (q09) once join
  bodies accept REAL columns.
- Answers move against SQLite, which sums in row order with KBN, in the last bits wherever KBN and
  the exact sum differ. The A/B should count how often on the corpus.

Tests for both arms: answers bit-equal to the serial loop (`format!("{:?}")` of the rows, as
t_qt_6_30 compares them) at 1, 2, 4, 8 and 16 stated cores, on data that stresses order:
magnitudes of 1e300 and 1e-300 mixed, integers past 2^53 before and after the first REAL, inf and
-inf (their sum is NULL), -0.0. For (b), also against an exact reference.

The measurement: bench targets for (a) and (b) and duckdb-mt, `--only q01,q06,q09 --cores
1,2,4,8,16 --runs 5`, the arms alternated; peak RSS from `/proc/<pid>/status` VmHWM; answers
against SQLite with `--answers`; load, CPU and I/O pressure beside each run (G6's `scale.sh` and
`summ.py` under ~/.cache/amphora-g6 on adacachy2 do most of it).

## Files

Outputs on adacachy2 under ~/.cache/amphora-g7: clippy-*.txt, tests-*.txt, t630-*.txt,
flaky-*.txt, three-*.txt, split-*.txt and t629-*.txt (the A/Bs above), bench-build-main.txt (r815's
failed build), and the test binaries in bin/. Worktrees under ~/.cache/columnar-ab. The experiment
arms are branches, not for main: perf/columnar-g7-null (deb631a0), perf/columnar-g7-plan
(4c0790c6), perf/columnar-g7-load (2ef87485).
