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

Timed A/B (perf-C2, 2026-11-23 01:46Z): `crates/engine/examples/versioned_point_reads.rs`, 10,000 rows,
autocommit `SELECT v FROM t WHERE id = ?` from each client's own connection, release builds of
b817508b (before) and cb47a7b5 (after: main with item 1, plus the CopyId change below), alternated
before/after per cell, three rounds, adacachy2 (32 cores). Its load average was 70 / 66 / 57 at the
three rounds' starts (other runs beside it), so the default storage's spread (0.8-2.3 M reads/s)
is this run's noise.

Command: `REPS=3 CLIENTS="1 64" READS=4000 STORAGES="versioned default" BASE_SHA=b817508b81a37fbc71e06be4ed1712fd92e888ef ./scripts/perf-versioned-ab.sh`

| storage, clients | before, reads/s (3 rounds) | after, reads/s (3 rounds) | after / before (medians) |
|---|---|---|---|
| versioned, 1 | 316, 102, 168 | 193,627, 147,109, 234,451 | ~1,150x |
| versioned, 64 | 3,663, 2,275, 2,311 | 481,712, 577,898, 489,583 | ~210x |
| default, 1 (control) | 1,165,197, 816,010, 860,102 | 977,236, 1,091,892, 1,122,122 | unchanged (noise) |
| default, 64 (control) | 1,472,108, 1,474,473, 1,436,985 | 1,837,853, 2,251,200, 1,585,398 | unchanged (noise; other lanes' main changes) |

The single-reader counter test establishes the redo work removed from this workload. The timed
harness released its workers before the reporter started its clock. Any reads completed in that
interval contributed to throughput with their elapsed time omitted. The after arm also included
the CopyId change. These rates cannot establish an isolated speedup for the unwritten transaction.
A reader finishing after another writer publishes may still need to flush that writer (T-T1.3.1).
The revised harness starts the clock before releasing the prepared workers. Each SELECT must
return its bound key's expected value exactly once, and the reported count sums those answers.

## Item 2, part: a leaf copy's id per thread (perf-C2, branch perf/versioned e7259743, not merged)

Verified: `vdbe/leaf_batch.rs` keys a cursor slot's decoded batch by `LeafId::Copy(copy)`, and a
slot can hold a later cursor in the same run, so the id must stay unique across cursors: per-cursor
counts would not do. `CopyId` is (the thread's number, its own count), a new thread number only on
a thread's first copy or a spent count: as unique, without each copy taking the shared COPIES line.
Test: `storage::versioned::cursor::copy_id_tests::audit_2_4_copy_ids_are_unique_across_threads`.

A/B (01:58Z), `REPS=3 CLIENTS="1 64" READS=20000 STORAGES=versioned BASE_SHA=dcd6ce4c ./scripts/perf-versioned-ab.sh`,
dcd6ce4c (main with item 1) vs e7259743, alternated, adacachy2 at load average 73 on 32 cores:

| clients | before, reads/s | after, reads/s |
|---|---|---|
| 1 | 124,143, 75,467, 148,158 | 73,619, 103,162, 157,141 |
| 64 | 182,450, 149,631, 404,313 | 175,883, 173,004, 518,567 |

Inconclusive: run-to-run noise (2.7x at 64 clients) is far above any effect, so it is not on main;
it stays on branch perf/versioned for an A/B on a quiet machine (or a counted one: cache-line
transfers under `perf stat -e`).

## Not done, for the successor (verified against the code)

- Access::Read for read-only statements: `Switch::cursor` (switch.rs) always passes
  `Access::Write(self.txn()?)`; its callers are `versioned_cursor` (vdbe/execute.rs, from OpenRead
  and OpenWrite). A cursor with `txn: None` keeps no reads (`VersionedCursor::note`). Under the
  switch's validation (`Validation::Precision`, the default; SQL never opens SSI) a transaction that
  only read commits unchecked and its reads are dropped (`Versioned::kept`), so an autocommit
  read-only statement's reads are pure cost. Hazards: one snapshot must serve every cursor of the
  statement (a join's two cursors), so take it once per statement (at the first cursor) and close
  it at the statement's end; an explicit transaction (BEGIN) must keep `Access::Write` (repeatable
  snapshot, own writes); the program must be known read-only (no OpenWrite) before its first cursor.
- The per-seek half-leaf copy, eager `join_chunks` of other rows, `pool.version()` per column
  (`fresh()`, cursor.rs) and collect()/checkpoint() on the switch path: unstarted.

## Known failures on main itself

`storage::versioned::columns_sql::tests::cb4_a_read_only_transaction_reads_the_copy_at_its_snapshot` and
`storage::versioned::columns_sql::tests::ab::cb4_ab_both_rebuy_policies_at_the_extremes` fail on
b817508b (before this change) at the same lines (columns_sql.rs:1263, 1875): "no generation answers
the statement's snapshot".
