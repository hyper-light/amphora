# perf/columnar, wave 1 (agent perf-G), 2026-11-22

Machine: kiln runner d1323898b255a3ed435eb83fa0f7b0cf (macos/arm64), placed by `run` with needs
`macos/arm64,cpus=8,mem=24g`. Command: `.perf-columnar/check.sh all` (rustc -C opt-level=3
-C overflow-checks=on microbench of the two code paths verbatim, then clippy and the engine's
morsel/columns unit tests).

## Audit 3 (columnar 3): ExactSum::add allocated a Vec<f64> per float (verified)

6,001,215 adds (SF1 lineitem's row count, prices 900.00-105,849.99), 7 reps, same process:

| | ns/row (min-max over 7 reps) |
|---|---|
| copying (before) | 24.55-25.06 |
| in place (after) | 11.30-11.57 |

2.2x on the per-row sum of a REAL SUM in a column copy; noise under 2%. Parts bit-identical
(test `audit_3_exact_sum_in_place_matches_copying`, 100k adds with overflow, NaN and infinity).

## Audit 7.7: MachineCosts::measure() started a thread per connection with parallel_mode off (verified, narrowed)

On the pager path the MorselAggregate opcode is only emitted with the switch on
(translate/mod.rs set_parallel); the column-copy path (versioned, copies on) emits it with the
switch off, and both its call sites (execute.rs op_morsel_aggregate, columns_sql.rs OnCores) then
measured: one scoped thread started and joined, plus available_parallelism. Measured cost of that
step: 14.7-20.2 us per connection's first such statement (5 reps of 2,000). After: off starts no
thread and reads no cores (`MachineCosts::for_mode`); on, costs never timed are measured once.
