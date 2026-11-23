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

## perf-C3 unit 1: a read-only end waits for every commit published before it (T-T1.3.1, AUDIT 2.4)

The independent review found that d4418ee0's read-only end flushed redo only through its own
snapshot (`txn.start`). A writer that published after the reader began, buffered (not yet
flushed), stayed unflushed when the reader reported a successful commit. The old path took a
timestamp after every publish and flushed through it. The fix is the reviewer's 67d1bab5
(t1/main-review), cherry-picked alone: flush through `clock.published()` at the end. Checked
against the code: `published()` only advances under the publish lock, after `redo.push` stored
`appended`; `Clock::recovered` and `Redo::new` both start at `replay.newest`. So a read-only
workload's end still costs one atomic load per lane, and only a new publish makes it flush.
Evidence: `audit_review_unwritten_commit_flushes_the_prefix_at_finish` (power cut after the
reader's end, reopen, the writer's row is there, one redo record). It failed before (reviewer's
r89) and passes on adacachy2 (r187: 153 passed, 2 failed = the known cb4_* below; on the merged
tip ac58af91, r207: 155 passed, the same 2 failed). On main as a07b635e and ac58af91.

## perf-C3 unit 2: a checkpoint flushes its pages before its header; a clean reopen skips its journal (AUDIT 7.5)

Ada's decision (2026-11-23): one more fsync per checkpoint, between the pages and the header.
The header carries a word after its stamp saying so. A reopen whose header has the word and
names the newest journal's checkpoint skips `Journal::apply`. An old-order header (no word) is
finished from its journal as before, once, and that writes a marked header.

Crash evidence (debug, simulated disk): `audit_7_5_a_crash_at_each_step_of_a_checkpoint_recovers_every_commit`
crashes after each operation of a checkpoint (journal, pages, flush, header, flush) under five
power-cut seeds, then reopens twice, checkpoints and reopens. Every commit's rows come back each
time. Pending: the mutant (the new flush between pages and header deleted) runs in kiln run r207 on
adacachy2 (its MUTANT_EXIT line). The test should fail there. If it passes, it lacks teeth. `audit_7_5_a_clean_reopen_writes_no_checkpoint_again`:
a clean reopen writes the data file 0 times and flushes it 0 times. Before, every reopen rewrote
every page in the journal and flushed once.

Reopen time, release, `crates/engine/examples/versioned_reopen.rs` (20,000 rows of 100 bytes,
one commit each, flushed and checkpointed; 20 reopens of the same files per rep; 5 reps
alternating before/after; before = b6cb6aa8, after = 8139baeb):

    OUT=$HOME/.cache/amphora-perf/c3-reopen; for rep in 1 2 3 4 5; do for side in before after; do
      $OUT/$side $OUT/db-$side-$rep 20000 20; done; done

| rep | before median (min-max), ms | after median (min-max), ms |
|---|---|---|
| 1 | 1,556 (717-4,708) | 84.0 (3.5-426) |
| 2 | 2,266 (960-3,708) | 91.4 (3.5-329) |
| 3 | 1,616 (649-2,946) | 132 (5.1-368) |
| 4 | 964 (249-1,350) | 78.1 (4.4-294) |
| 5 | 864 (588-1,240) | 6.9 (2.7-150) |

Verdict: faster. Every after median sits below every before minimum. The machine was loaded
(load average 74 on 32 cores, including this lane's own test build), so the ratio itself
(roughly 12x-125x by median) is not a stable number. The limiter of "before" is inferred, not
profiled: an fsync of the rewritten pages on a contended NVMe. Every reopen on both sides read
back all 20,000 rows. To repeat on a quiet machine: the command above.

The original example printed the number of rows it scanned. It accepted any count or contents.
In verifier run r30 on adacachy2, requesting 16 rows after a 32-row run on the same path
succeeded with `rows_read=[32, 32]`. With zero reopens it succeeded with three NaNs.
The corrected example uses a fresh directory on the same device. It checks each expected key
and its 100-byte value after timing each open and rejects an empty workload. The five pairs
above remain the original measurements. The retained `before.log` and `after.log` in the
named directory contain compiler output. Neither contains the per-reopen measurements.

## Not done, for the successor (verified against the code)

- Access::Read for read-only statements (perf-C3 unit 3, not started): `Switch::cursor` (switch.rs) always passes
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
