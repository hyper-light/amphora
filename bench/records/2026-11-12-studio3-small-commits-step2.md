# Small durable commits on a Mac Studio: decision 37 (step 2) against 13e385d (2026-11-12)

Kiln goal 60805ec4, part B, step 2 (docs/design/store.md decision 37; goal 370cb46a). The unify
audit's legs and the comprehensive suite's write statements, as
`bench/records/2026-11-07-studio3-small-commits.md` measured step 1, on three trees: 13e385d (base,
page-a-frame), da367e6 (step 1, decision 35) and 4acb85d (step 2: a commit logs its pages' changes in
a mixed log, decision 37, with a record of images alone stored, not LZ4'd).

**The load gate was off for this run, on Ada's word (2026-11-11: "Just run the fucking bench
christ").** Each phase began after three samples 30 s apart, whatever the load, with no check for
compilers; each phase's load was printed and is below. Several phases began at a load over 2, so
this run does not meet CLAUDE.md section 7's bar of a load under 2. Its numbers are what was
measured, under the loads shown.

- host: adas-mac-studio-3 (kiln runner 2768d0d5d2974f1e21d348edadff9d1b; `hostname` said
  Adas-Mac-Studio-4.local), Mac Studio, Apple M4 Max, 14 cores, 36 GiB, APFS on its internal SSD
  (94% full, 27 GiB free, read after the run), macOS 26.2, rustc 1.99.1 (58cf8f9c3 2026-10-13).
- date: 2026-11-12, 03:52:06Z to 04:14:54Z.
- trees: each the commit's tree exactly, checked by `git write-tree` when made (2026-11-08) and kept
  by hash since: step 2 4acb85d (tree 09ceb0ab4d6b682a759ecad817eca1b7fd1644ae), step 1 da367e6
  (d42fbd9ea5b41e03606f9fd85721c2cab1fa5d57), base 13e385d (f32571349438798416d54a12f5a2f84de0443198),
  with the audit's harness copied in: `crates/zz-audit` (main.rs 08f83d63df096ae0, batched.rs
  a298f9aafd83e736) and its corpus (`kiln-unify-logs.txt`, dcc7c852d76f6b42), the same as the
  2026-11-06 and 2026-11-07 records.
- binaries: release, built 2026-11-08 03:12-03:28Z on this host and kept (sha256 prefixes): `zz-audit`
  46eac9428e277826 (step 2), beaef016d74b4017 (step 1), 07fa057f3714c724 (base); `amphora-bench`
  ea68e8e04ce9ba89 (step 2), 542b63ec10b86158 (base).
- command: a kiln background run on that runner, in the synced copy of
  `~/.cache/claude-scratch/wal-redo-carry` (its git repo at 4c34288, with `bench-l2.sh` changed to
  read `CALM_LOAD` and `NO_BUILD_CHECK`, uncommitted; sha256 cf1986589214dff1):

  ```
  CALM_LOAD=1000 NO_BUILD_CHECK=1 bash ./bench-l2.sh 09ceb0ab4d6b682a759ecad817eca1b7fd1644ae d42fbd9ea5b41e03606f9fd85721c2cab1fa5d57 f32571349438798416d54a12f5a2f84de0443198 4acb85d
  ```

  It held the host's heavy lock (`~/.cache/amphora-heavy.lock`, gone after the run), then ran two
  rounds of step 2, step 1, base, each:

  ```
  zz-audit legs <dir> kiln-unify-logs.txt 120 1 4 8
  zz-audit batched <dir> kiln-unify-logs.txt 1
  ```

  then, once for step 2 and once for base:

  ```
  amphora-bench --suite --target amphora --dir <dir> --items 200000 --clients 1,32 --seconds 5 --warmup 1 --only insert_one,insert_batch_100,update_plain,update_indexed,update_text,upsert,delete_range_insert,txn_read_write
  ```

  "Disks wrote" is the change in `IOBlockStorageDriver`'s bytes written across a run: everything the
  machine wrote, not only Amphora. The harness's own storage counter (`/proc/self/io`) reads 0 on
  macOS. The outputs are on the host: `~/.cache/wal-redo-bench-l2.log` (this run's lines begin at
  2026-11-12T03:52:06Z), `.log.build`, `.log.suite`, and `~/.cache/wal-redo-bench-l2-suite-{step2,base}.jsonl`.
- best of N: N = 2 for the audit's legs (the two rounds). N = 1 for the suite: each statement ran
  once per tree, 5 s after 1 s of warm-up.
- the run completed: every phase ran, both suite runs exited 0, and the log ends with "done".

## Each phase's load (one-, five- and fifteen-minute averages)

| phase | at its start | at its end |
|---|---|---|
| measuring began | 1.11 1.39 1.37 | |
| round 1 step 2 legs | 1.16 1.37 1.36 | 2.33 1.60 1.44 |
| round 1 step 2 batched | 1.30 1.44 1.39 | 1.40 1.45 1.40 |
| round 1 step 1 legs | 0.99 1.33 1.36 | 1.15 1.34 1.36 |
| round 1 step 1 batched | 3.01 1.83 1.53 | 2.50 1.79 1.53 |
| round 1 base legs | 2.01 1.79 1.54 | 1.72 1.74 1.53 |
| round 1 base batched | 1.80 1.72 1.53 | 2.33 1.84 1.58 |
| round 2 step 2 legs | 2.62 1.98 1.65 | 3.20 2.19 1.73 |
| round 2 step 2 batched | 2.30 2.12 1.74 | 1.93 2.05 1.72 |
| round 2 step 1 legs | 1.52 1.91 1.69 | 1.55 1.89 1.69 |
| round 2 step 1 batched | 1.36 1.74 1.64 | 1.81 1.82 1.67 |
| round 2 base legs | 1.13 1.62 1.61 | 3.14 2.06 1.76 |
| round 2 base batched | 13.20 4.88 2.82 | 15.01 5.68 3.14 |
| suite, step 2 | 9.94 6.18 3.52 | (not printed) |
| suite, base | 3.71 4.90 3.44 | (not printed) |
| the run's end | | 9.19 6.44 4.19 |

Round 1 ran at loads from 0.99 to 3.01, and round 2 from 1.13 to 3.20 up to round 2's base legs.
Something outside the run then loaded the host: round 2's base legs had the disks write 1,979 MiB,
where the same legs wrote 614 in round 1, and the load stood at 13.20 when round 2's base batched leg
began. This run did not record what that was. Both suite runs began under it (9.94 and 3.71).

## Many small commits: the batched leg

1 MiB of log text: 157 documents, 1,665 posting calls of 128 grams, each waiting until durable;
27,914 posting rows at the end on every tree.

| tree | time | call p50 | call p99 | disks wrote for 1 MiB of text | file, log at the end |
|---|---|---|---|---|---|
| base, round 1 | 16.6 s | 9.94 ms | 13.98 ms | 633 MiB | 4 MiB, 208 MiB |
| base, round 2 | 13.6 s | 7.97 ms | 11.98 ms | 595 MiB | 4 MiB, 208 MiB |
| step 1, round 1 | 16.8 s | 9.95 ms | 14.93 ms | 417 MiB | 4 MiB, 208 MiB |
| step 1, round 2 | 16.9 s | 9.95 ms | 15.05 ms | 420 MiB | 4 MiB, 208 MiB |
| step 2, round 1 | 16.9 s | 9.95 ms | 13.92 ms | 218 MiB | 0 MiB, 24 MiB |
| step 2, round 2 | 16.2 s | 9.95 ms | 15.10 ms | 217 MiB | 0 MiB, 24 MiB |

Step 2 had the disks write 217-218 MiB for its MiB of text, against 595-633 for base and 417-420 for
step 1: 2.7 to 2.9 times less than base. Its log ended at 24 MiB, where base's and step 1's ended at
208. Its checkpoint copied nothing into the file during the leg (the file at 0 MiB).

Decision 37's bound is stated in bytes a byte of rows: at most 4 for the log, at most 5 for all that
is written, against 16.6 and 26.4 before (`small_commits_log_their_changes_not_their_pages` counts it
in the test). This run doesn't count the bytes of rows the audit leg's calls commit, so it can't give
that ratio. In the disks' terms, decision 35's figures for this leg give the bound the same way they
gave step 1's 481 MiB. Decision 35 puts the page-a-frame log at about 232 MiB, which is 16.6 bytes a
byte of rows. At most 4 bytes a byte of rows is therefore at most 56 MiB of log. At most a quarter of
that copied into the file adds 14 MiB. The per-flush floor of about 107 KiB for each of the 1,822
durable flushes adds 191 MiB (`F_FULLFSYNC`, measured on this host's APFS). That gives at most
261 MiB. Step 2 wrote 217-218 MiB, within that bound. 191 MiB of that bound is the flush floor, which
no format of the log removes.

## One big transaction: the 1, 4 and 8 MiB legs

| leg | base, rounds 1 and 2 | step 1 | step 2 | step 2, best against base's best |
|---|---|---|---|---|
| 1 MiB | 1.19 s, 1.21 s | 1.20 s, 1.19 s | 1.19 s, 1.31 s | the same (1.19 s) |
| 4 MiB | 6.34 s, 7.21 s | 6.32 s, 6.37 s | 6.34 s, 7.32 s | the same (6.34 s) |
| 8 MiB | 13.80 s, 15.95 s | 13.94 s, 13.94 s | 14.04 s, 16.34 s | 1.7% slower (14.04 s against 13.80 s) |

| | 1 MiB | 4 MiB | 8 MiB | disks wrote, the three legs, rounds 1 and 2 |
|---|---|---|---|---|
| base, database and log grew | 39.9 MiB | 178.2 MiB | 360.9 MiB | 614 MiB, 1,979 MiB |
| step 1 | 26.7 MiB | 178.4 MiB | 361.0 MiB | 617 MiB, 615 MiB |
| step 2 | 26.9 MiB | 179.3 MiB | 363.0 MiB | 757 MiB, 676 MiB |

At 8 MiB, step 2 was slower than base in each round: 14.04 against 13.80 s in round 1, and 16.34
against 15.95 s in round 2. At 1 and 4 MiB, step 2's best equals base's. Its round 2 was slower than
base's round 2 at every size (1.31 against 1.21 s, 7.32 against 7.21 s); step 2's round 2 legs began
at load 2.62. Step 2's database and log now grow as base's do (363.0 MiB at 8 MiB, against 296.2 MiB
at 40c9c84 in the 2026-11-07 record), since 4acb85d stores a record of images alone without LZ4. Its
disks wrote 757 and 676 MiB across the three legs, against base's 614 in round 1.

## The comprehensive suite's write statements

One run per tree, 200,000 items, 5 s each. Step 2's run began at load 9.94 and base's at 3.71. Writes
a second, and the median latency:

| statement | clients | step 2 | base (13e385d) | step 2 against base | p50, step 2 / base |
|---|---|---|---|---|---|
| insert_one | 1 | 213.0 | 215.0 | -0.9% | 4.86 / 4.10 ms |
| insert_one | 32 | 4,780.8 | 6,214.4 | -23.1% | 6.78 / 4.99 ms |
| insert_batch_100 | 1 | 81.8 | 151.4 | -46.0% | 11.78 / 6.02 ms |
| insert_batch_100 | 32 | 85.6 | 217.6 | -60.7% | 380.93 / 139.26 ms |
| update_plain | 1 | 221.2 | 188.0 | +17.7% | 4.06 / 4.99 ms |
| update_plain | 32 | 6,976.0 | 4,985.6 | +39.9% | 4.03 / 5.95 ms |
| update_indexed | 1 | 232.8 | 184.2 | +26.4% | 4.00 / 4.99 ms |
| update_indexed | 32 | 6,153.4 | 2,937.6 | +109.5% | 4.99 / 8.58 ms |
| update_text | 1 | 234.4 | 192.6 | +21.7% | 4.00 / 4.99 ms |
| update_text | 32 | 6,720.0 | 4,631.6 | +45.1% | 4.03 / 5.89 ms |
| upsert | 1 | 234.8 | 168.2 | +39.6% | 4.00 / 5.44 ms |
| upsert | 32 | 7,737.6 | 4,985.6 | +55.2% | 4.86 / 5.89 ms |
| delete_range_insert | 1 | 171.6 | 138.2 | +24.2% | 5.18 / 5.95 ms |
| delete_range_insert | 32 | 614.2 | 752.2 | -18.3% | 40.96 / 28.67 ms |
| txn_read_write | 1 | 177.6 | 191.4 | -7.2% | 4.99 / 4.99 ms |
| txn_read_write | 32 | 2,969.6 | 2,924.8 | +1.5% | 8.96 / 7.94 ms |

No errors, retries or timeouts on either tree. The suite's store counters across all sixteen runs:
step 2 committed 22,534 times in 16,488 flushes and 1,184 checkpoints, with a peak heap of 74.6 MB;
base committed 18,557 times in 13,779 flushes and 1,047 checkpoints, with a peak heap of 21.2 MB.

The single-row updates and the upsert ran 18-110% faster on step 2. The statements that write many
pages in one commit ran slower: `insert_batch_100` 46% slower at 1 client and 61% slower at 32, and
`delete_range_insert` 18% slower at 32 clients. `insert_one` ran 23% slower at 32 clients. This is
the shape the packed log showed in 2026-10-22's comprehensive bench (decision 16: `insert_batch_100`
at 32 clients 392 to 44 a second). Each figure is one run, and step 2's run began under more than
twice base's load.

## Goal 370cb46a, criterion by criterion

1. **The batched leg's log at or under decision 37's bound: met on the disks' counter.** The disks
   wrote 217-218 MiB for the MiB of text, against the 261 MiB that decisions 35 and 37's figures give
   (base 595-633, step 1 417-420). The log ended at 24 MiB against base's 208. The bound's own
   measure, bytes of log a byte of rows, is not counted by this run on macOS.
2. **The 1, 4 and 8 MiB legs no slower than at 13e385d: not met at 8 MiB.** Best of 2, step 2 took
   14.04 s against 13.80 s (1.7% slower), and it was slower in each round. At 1 MiB (1.19 s) and
   4 MiB (6.34 s), step 2's best equals base's.
3. **The comprehensive bench's bulk statements no slower than at 13e385d: not met.** `insert_batch_100`
   ran 81.8 against 151.4 a second at 1 client and 85.6 against 217.6 at 32. `delete_range_insert`
   at 32 clients ran 614.2 against 752.2. These are one run each, begun at loads 9.94 (step 2) and
   3.71 (base).

Criteria 2 and 3 are conditions that decision 37's "Falsified by" names: the audit's legs, or the
comprehensive bench's bulk statements, slower than page-a-frame at 13e385d. As measured here, both
conditions hold. The run was not on a quiet host, and the suite was not best of N.
