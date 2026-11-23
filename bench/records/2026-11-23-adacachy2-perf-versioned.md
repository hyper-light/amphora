# perf/versioned (AUDIT 2.4, 6, 7.5): 2026-11-23

## Item 1: an unwritten autocommit transaction ends without a commit (main d4418ee0, change 5c125f1c)

Counted A/B, the test `audit_2_4_read_only_autocommit_logs_and_flushes_nothing`
(`crates/engine/storage/versioned/switch.rs`), adacachy2, debug build:

| | before (b817508b) | after (5c125f1c) |
|---|---|---|
| redo records per autocommit SELECT | 1 (28 bytes) | 0 |
| lane flushes (write_sync) per autocommit SELECT | 1 | 0 |
| publish lock per autocommit SELECT | 1 | 0 |
| records / flushes per autocommit INSERT | 1 / 1 | 1 / 1 |

Timed A/B: not run. `scripts/perf-versioned-ab.sh` builds `crates/engine/examples/versioned_point_reads.rs`
at the base and at HEAD (release) and runs both storages at 1, 8, 32 and 64 clients, three rounds,
alternating before/after. The macOS runner refused the sync twice (`did not answer the sync in 60s`) and
adacachy2 had 2 of 32 cores idle at 01:05Z, so no run met CLAUDE.md §7's load bar.

Command: `BASE_SHA=b817508b81a37fbc71e06be4ed1712fd92e888ef ./scripts/perf-versioned-ab.sh`

## Known failures on main itself

`storage::versioned::columns_sql::tests::cb4_a_read_only_transaction_reads_the_copy_at_its_snapshot` and
`storage::versioned::columns_sql::tests::ab::cb4_ab_both_rebuy_policies_at_the_extremes` fail on
b817508b (before this change) at the same lines (columns_sql.rs:1263, 1875): "no generation answers
the statement's snapshot".
