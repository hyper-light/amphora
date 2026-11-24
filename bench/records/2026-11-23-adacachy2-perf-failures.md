# 2026-11-23, adacachy2: lane K10, the two engine failures left after the lint gate

Runs: kiln `run` on adacachy2 (runner 00aad8da), needs `linux/amd64,cpus=4,mem=16g`,
CARGO_INCREMENTAL=0, CARGO_BUILD_JOBS=4, one target dir. Each arm's engine lib test binary is kept
under `~/.cache/amphora-perf-bin/failures-k10/<sha>`.

## 1. `skipping::tests::versioned::t_ns_2_1_stale_against_wait_under_writers`

The test alone (`--exact`, `--test-threads=1`), 10 runs per arm:

| Build | Failed | Where |
|---|---|---|
| 9a7b14bb (main before lane Q3's rewrites under `skipping/`, kiln r1305) | 10/10 | writer's INSERT unwrap, tests.rs:3783 |
| 93b6458c (Q3's gate on main, kiln r1305) | 10/10 | the same |
| 40147506 (this lane, kiln r1348) | 0/10 | |

So the gate's change didn't cause it. The writers' autocommit INSERTs are refused with
`SerializationFailure`, which `docs/design/engine.md` decision 5 says the caller retries. The
mechanism is read from the code, not traced: a writer's rowid probe keeps the span from its rowid
to the next row or to the end (`storage/versioned/cursor.rs`, module doc), so a higher rowid
committed by the other writer since its snapshot refuses it under precision locking. With the
writers retrying, each arm saw 1 to 7 refusals per run (Stale 1-3, Wait 2-7 across the 10 runs),
every run ended, and the table held all 3000 + writers * txns rows.

Open: an exact-rowid probe that misses could keep the point instead of the span to the next row,
which would cut these refusals. That is a validation change needing Elle's checks, not done here.

## 2. `vdbe::insn::error_size_tests::limbo_error_stays_small`

93b6458c: `size_of::<LimboError>() <= 40` failed. `MemoryBudget { what: &'static str, asked,
held, limit: u64 }` is a 40-byte payload, so the enum was 48 bytes (G8 step 2, 786e8f31). Boxed
as `MemoryBudget(Box<BudgetFull>)`: the test passes on 40147506.

## Suites on 40147506 (kiln r1348)

`skipping::` 72 passed; `storage::versioned::{memory, pool, evict, columns::, version::}` all
passed; `vdbe::insn` passed; clippy `-p amphora-engine -p amphora --all-targets --locked -D
warnings` exit 0. `storage::versioned::columns_sql` 10 passed, 2 failed:
`cb4_a_read_only_transaction_reads_the_copy_at_its_snapshot` (columns_sql.rs:1267, "the copy
didn't answer") and `ab::cb4_ab_both_rebuy_policies_at_the_extremes` (columns_sql.rs:1879). Both
fail 3/3 on 93b6458c too, with the same counts, so they predate this lane.
