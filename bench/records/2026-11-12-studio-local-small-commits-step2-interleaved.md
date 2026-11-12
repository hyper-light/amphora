# Small durable commits, step 2 against 13e385d, interleaved (2026-11-12)

Kiln goal 60805ec4, part B, step 2 (docs/design/store.md decision 37; goal 370cb46a). This reruns
`bench/records/2026-11-12-studio3-small-commits-step2.md`'s criteria 2 and 3. That run ran each tree's
suite once, step 2 first, at loads of 9.94 and 3.71. Here the two trees are interleaved: each
statement runs on base and then on step 2, in three rounds, so both trees see the same load. Each
statement is its own call, so the store's counters are that statement's. One profiled call per tree
of the bulk statements follows, kept out of the timed results.

**The load gate was off, on Ada's word ("Just run the bench").** Every run's load at its start is
below. None began under 2: the one-minute load ran from 2.10 to 6.57, so this run does not meet
CLAUDE.md section 7's bar of a load under 2. Interleaving is what makes the comparison fair: each
pair of runs began within 16 s of each other.

- host: adas-mac-studio.local (kiln runner cec37f60f22c5a60d27f08f4d2e5daa8; `hostname` said
  Mac.attlocal.net during the run and Adas-Mac-Studio-3.local an hour before), Mac Studio, Apple M4
  Max, 14 cores, 36 GiB, APFS on its internal SSD (6 GiB free, 99% full), macOS 26.2, rustc 1.99.1
  (58cf8f9c3 2026-10-13).
- date: 2026-11-12, 07:13:08Z to 07:26:54Z.
- trees: step 2 4acb85d (tree 09ceb0ab4d6b682a759ecad817eca1b7fd1644ae) and base 13e385d (f32571349438798416d54a12f5a2f84de0443198), as made by
  `bench-l2.sh` on this host. Each was checked file by file against its commit's `git ls-tree -r`
  before the run (`tree-check.sh`): every file is the commit's blob except what the bench adds. That
  is the audit's harness (`crates/zz-audit`, `walrepro/`, `.tree`), the lockfiles its builds wrote
  (`Cargo.lock` with zz-audit in the workspace, `bench/Cargo.lock`), and on base `bench/Cargo.toml`,
  which is the commit's file with the bench's `[patch.crates-io]` appended, byte for byte.
- binaries: release, already built on this host; `cargo build --release` compiled no crate before
  the run. sha256 prefixes: `amphora-bench` b0451b9203d30492 (step 2), e78c467fa16b8298 (base);
  `zz-audit` 5ffb629b35ef74ff (step 2), 0a977fd6f7780947 (base). Harness main.rs 08f83d63df096ae0,
  corpus dcc7c852d76f6b42, as in the earlier records.
- command: a kiln background run on that runner, in the synced copy of
  `~/.cache/claude-scratch/wal-redo-carry` at e301082 (`rerun-s2.sh`, sha256 cc5250a182955ef9):
  `bash ./rerun-s2.sh`. It held the host's heavy lock (gone after the run). For each of three
  rounds, it ran each statement on base and then on step 2:

  ```
  amphora-bench --suite --target amphora --dir <db> --items 200000 --clients 1,32 --seconds 5 --warmup 1 --only <statement>
  ```

  The statements, in order, were insert_batch_100, delete_range_insert, insert_one, update_plain,
  update_indexed, update_text, upsert and txn_read_write. Then came the 8 MiB leg, on base and then on
  step 2:

  ```
  zz-audit legs <dir> kiln-unify-logs.txt 120 8
  ```

  After the three rounds, one profiled call of insert_batch_100 and of delete_range_insert ran per
  tree, interleaved the same way:

  ```
  AMPHORA_PROFILE=<prefix> amphora-bench ... --only <statement>
  ```

  The profiler samples each thread's CPU 997 times a second; time off the CPU, such as a wait on a
  flush, isn't sampled. The outputs are on the host in `~/.cache/wal-redo-rerun/`: `log`,
  `r<round>-<tree>-<statement>.json`, `r<round>-<tree>-leg8.txt`, and `prof/` with `.folded`, `.txt`
  and `.svg` for each statement and client count.
- best of N: N = 3 for every timed figure. No run failed, and no statement had an error, retry or
  timeout.

## The suite's write statements, writes a second

| statement | clients | base, rounds 1-3 | step 2, rounds 1-3 | base best | step 2 best | step 2 against base, best | rounds step 2 was slower |
|---|---|---|---|---|---|---|---|
| insert_batch_100 | 1 | 122.2, 115.6, 125.4 | 80.0, 70.2, 42.8 | 125.4 | 80.0 | -36.2% | 3 of 3 |
| insert_batch_100 | 32 | 216.4, 42.2, 544.2 | 86.8, 92.8, 45.2 | 544.2 | 92.8 | -82.9% | 2 of 3 |
| delete_range_insert | 1 | 142.6, 140.2, 145.8 | 146.8, 161.8, 152.8 | 145.8 | 161.8 | +11.0% | 0 of 3 |
| delete_range_insert | 32 | 1,184.0, 3,056.8, 3,082.6 | 747.4, 974.4, 963.6 | 3,082.6 | 974.4 | -68.4% | 3 of 3 |
| insert_one | 1 | 143.6, 172.2, 161.0 | 160.2, 154.4, 149.4 | 172.2 | 160.2 | -7.0% | 2 of 3 |
| insert_one | 32 | 3,520.0, 1,766.4, 3,718.4 | 3,872.0, 5,017.6, 4,872.6 | 3,718.4 | 5,017.6 | +34.9% | 0 of 3 |
| update_plain | 1 | 152.6, 164.6, 170.6 | 140.2, 147.4, 147.8 | 170.6 | 147.8 | -13.4% | 3 of 3 |
| update_plain | 32 | 4,198.4, 5,019.2, 4,262.4 | 4,524.8, 5,113.6, 4,934.8 | 5,019.2 | 5,113.6 | +1.9% | 0 of 3 |
| update_indexed | 1 | 133.2, 144.4, 146.0 | 152.8, 149.6, 152.4 | 146.0 | 152.8 | +4.7% | 0 of 3 |
| update_indexed | 32 | 4,633.6, 5,177.6, 5,292.8 | 4,755.2, 5,356.8, 5,209.6 | 5,292.8 | 5,356.8 | +1.2% | 1 of 3 |
| update_text | 1 | 144.4, 160.0, 163.0 | 153.4, 151.2, 146.2 | 163.0 | 153.4 | -5.9% | 2 of 3 |
| update_text | 32 | 4,588.8, 5,100.8, 5,056.0 | 3,808.0, 5,075.2, 4,544.0 | 5,100.8 | 5,075.2 | -0.5% | 3 of 3 |
| upsert | 1 | 164.6, 165.4, 166.6 | 136.4, 150.2, 146.4 | 166.6 | 150.2 | -9.8% | 3 of 3 |
| upsert | 32 | 4,435.2, 5,196.8, 5,030.4 | 4,108.8, 4,930.2, 4,940.8 | 5,196.8 | 4,940.8 | -4.9% | 3 of 3 |
| txn_read_write | 1 | 136.0, 164.6, 166.4 | 136.4, 145.4, 70.8 | 166.4 | 145.4 | -12.6% | 2 of 3 |
| txn_read_write | 32 | 3,820.0, 5,549.8, 5,273.6 | 3,328.0, 1,088.0, 4,147.0 | 5,549.8 | 4,147.0 | -25.3% | 3 of 3 |

The one-minute load at the start of the bulk statements' runs, base / step 2:

| | round 1 | round 2 | round 3 |
|---|---|---|---|
| insert_batch_100 | 3.13 / 2.80 | 3.00 / 3.16 | 3.71 / 6.57 |
| delete_range_insert | 2.95 / 2.43 | 3.13 / 2.96 | 5.76 / 4.71 |

The other statements began at 2.10 to 5.57 (`log`). In round 3, step 2's insert_batch_100 began at
6.57 against base's 3.71. Its 42.8 and 45.2 a second are its worst; its best are from rounds 1 and 2,
which began at about the same load as base's (2.80 and 3.16 against 3.13 and 3.00). Base's insert_batch_100 at 32 clients ranged from 42.2 to
544.2. In round 2, base's 42.2 came with a 4.2 s checkpoint and a 0.67 s restart of the log (the
counters' longest), and was the one round in which step 2 was faster.

The single-row statements that ran 18-110% faster on step 2 in the earlier, uninterleaved run did
not do so here. Interleaved, their best figures are within -13% and +35% of base's. Step 2 was
slower at 1 client in most rounds (update_plain, upsert, update_text) and faster at 32 clients only
in insert_one. The earlier speedups were the earlier run's order and load, not step 2.

## The 8 MiB leg

| round | base | step 2 | step 2 against base | load at the start, base / step 2 | disks wrote, base / step 2 |
|---|---|---|---|---|---|
| 1 | 14.49 s | 14.51 s | +0.1% | 3.01 / 2.92 | 385 / 506 MiB |
| 2 | 14.28 s | 14.25 s | -0.2% | 4.07 / 4.03 | 388 / 387 MiB |
| 3 | 14.60 s | 14.74 s | +1.0% | 5.57 / 4.95 | 391 / 501 MiB |
| best of 3 | 14.28 s | 14.25 s | the same (-0.2%) | | |

Interleaved, step 2's 8 MiB leg is no slower than base's: within 1% in each round, and the same at
best. The earlier record's 14.04 s against 13.80 s (1.7% slower) ran step 2 and base 5 to 6 minutes
apart; this run puts them 16 s apart.

## Where step 2's time goes on the bulk statements

**The store's counters** cover each statement's call: 1 s of warm-up and 5 s at 1 client, then the
same at 32 clients. "In commits" is the time the writer spent running the batches' statements and
committing them (`Counters::apply_ms`).

| | commits | in commits | per commit | flushes | flushing |
|---|---|---|---|---|---|
| insert_batch_100, base, rounds 1-3 | 1,465, 1,123, 2,516 | 5.39, 3.29, 6.89 s | 3.7, 2.9, 2.7 ms | 780, 744, 950 | 5.01, 4.71, 4.13 s |
| insert_batch_100, step 2 | 753, 737, 433 | 9.52, 9.50, 9.71 s | 12.6, 12.9, 22.4 ms | 517, 471, 321 | 2.21, 2.33, 4.23 s |
| delete_range_insert, base | 1,618, 2,452, 2,473 | 3.51, 4.08, 4.05 s | 2.2, 1.7, 1.6 ms | 1,234, 1,421, 1,463 | 7.45, 6.05, 6.12 s |
| delete_range_insert, step 2 | 1,249, 1,370, 1,305 | 5.42, 7.00, 6.89 s | 4.3, 5.1, 5.3 ms | 1,055, 1,342, 1,285 | 5.69, 5.28, 5.28 s |

Step 2's writer spent 9.5-9.7 s of insert_batch_100's 12 s inside commits, 3.4 to 8 times base's
time a commit, while it flushed less. The flush is not where step 2 loses. The writer running the
statements is. delete_range_insert shows the same, at 2 to 3 times base's time a commit. Step 2's
peak heap was 54-56 MB on insert_batch_100, against base's 21-22 MB.

**The profile** of the writer's thread (its CPU samples, one profiled call per tree, at 32 clients
unless named). Shares are of the writer's samples; a frame's share includes what it calls.

| | base | step 2 |
|---|---|---|
| insert_batch_100: writer samples | 3,786 | 5,015 |
| a page read past the cache (`read_page_no_cache`) | 68.4% | 84.9% |
| of which, a page frame (`begin_read_wal_frame`) | 65.9% | |
| of which, a mixed log's record (`begin_read_record`) | | 82.9% |
| in `pread` | 68.7% | 77.7% |
| the commit (`commit_txn_wal`) | 27.6% | 13.4% |
| of which, its frames (`prepare_frames`) or records (`prepare_records`) | 1.6% | 12.8% |
| insert_batch_100 at 1 client: page read / of which a record / the commit / its records | 71.0% / (frames 48.5%) / 25.2% / 0.7% | 66.3% / 52.7% / 27.4% / 25.3% |
| delete_range_insert: writer samples | 2,415 | 5,298 |
| a page read past the cache | 70.7% | 80.6% |
| of which, a page frame / a record | 46.8% | 72.0% |
| in `pread` | 71.0% | 74.7% |
| the commit / its frames or records | 21.3% / 0.2% | 17.5% / 16.4% |

The profiled calls' own rates: insert_batch_100 base 120.4 and 539.0 a second at 1 and 32 clients,
step 2 18.4 and 65.4; delete_range_insert base 148.0 and 2,967.4, step 2 167.4 and 969.2.

What this shows, in step 2's commit path at 4acb85d:

1. **A page the writer reads past the page cache, from a record.** This is most of the writer's CPU:
   83% of insert_batch_100's samples at 32 clients and 72% of delete_range_insert's are under
   `WalFile::begin_read_record`. Nearly all of that is the `pread` beneath it; the profiler names the
   leaf below `pread` `_open$NOCANCEL`. Decoding the record (LZ4 and deltas, `delta`) is 0.0-0.8% of
   samples. In a mixed log, `RecordRead::issue` (`crates/engine/storage/wal.rs:614`) reads a page
   past the cache by reading the whole record that holds it: one `pread` of the record's slots times
   the frame's size. It does the same for each record of the page's chain not already decoded in the
   record registry. A page frame's read (`begin_read_wal_frame`) reads its one frame. Base's writer
   reads past the cache as well (66% of its samples), so the misses are there on both trees. What
   step 2 changes is what a miss reads. This run doesn't count the bytes those reads took, the
   records' sizes, or how often a decoded record was found in the registry.
2. **Making the commit's records** (`prepare_records`): 12.8-25.3% of the writer's samples on
   insert_batch_100 and 16.4% on delete_range_insert. Base's page frames (`prepare_frames`) took
   0.2-1.6%.

## Goal 370cb46a, criterion by criterion, with this run

1. **The batched leg's log within decision 37's bound: not re-measured here.** The earlier record's
   verdict stands: met on the disks' counter (217-218 MiB against 261).
2. **The 1, 4 and 8 MiB legs no slower than at 13e385d: met at 8 MiB.** Best of 3, step 2 took
   14.25 s against 14.28 s, within 1% in each round. The earlier record's 1.7% slower doesn't hold
   when the trees are interleaved. The 1 and 4 MiB legs were met in the earlier record and not rerun.
3. **The comprehensive bench's bulk statements no slower than at 13e385d: not met. The slowdown
   holds.** Best of 3, `insert_batch_100` ran 80.0 against 125.4 a second at 1 client (-36%, slower
   in every round) and 92.8 against 544.2 at 32 (-83%, slower in 2 of 3 rounds).
   `delete_range_insert` at 32 clients ran 974.4 against 3,082.6 (-68%, slower in every round). At 1
   client, `delete_range_insert` ran 161.8 against 145.8 (+11%).

Decision 37's falsifier, "the comprehensive bench's bulk statements slower than page-a-frame at
13e385d", holds on this run, interleaved and best of 3. The cost is in the writer: a page read past
the cache reads a whole record, and the commit packs its records. It is not in the flush or the
log's bytes.
