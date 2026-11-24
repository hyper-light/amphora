# Lane Q5: decision 40's frame lookup chosen in K0's mixed phase, the other ways deleted (adacachy2, 2026-11-24)

Deliverable status, losses first.

- Thin: three timed rounds a cell, not five, inside the hour. The profiles (two rounds) carry the
  choice; the throughputs agree with them where they separate and tie where they don't.
- Noisy host: adacachy2 ran other lanes' work throughout, 1-minute load 49-68 on 32 cores, CPU
  pressure (some, avg10) 36-54%. The ways were alternated so each saw the same noise.
- SQLite can't open Amphora's files: the log's header carries Amphora's own format version
  (`AMP` 7 where SQLite's 3007000 goes), so SQLite answers `unable to open database file`. The first
  answer check (in `q5_mixed.sh`) failed that way for every run and checked nothing. The answers are
  held to SQLite's on the same rows instead (`q5_answers.sh`, below).
- Fixed: K0's mixed phase opens again on `--target amphora`. Its engine (`Convo`, bench/src/store.rs)
  kept no group holding, which `Groups::start` asks every engine for since CP-T1.9; the suite's engine
  got one in 774f3d37 and K0's never did. Both now keep it through one pair of functions. Test:
  `store::tests::q5_k0_amphora_opens_and_reads_what_it_wrote`.
- Kept: `optimistic`, the sharded table of each page's latest frame read under version checks.
  Deleted: `LogReadIndex`, `AMPHORA_LOG_READ_INDEX`, `set_process_default`, `ReadIndex::set_mode`,
  the published copy and its lock, and the index callback's high-water argument only it read.

## What was measured

One binary, `amphora-bench` at b6e2e72f (this lane's branch: main 1882e8c6 plus the holding fix and
`AMPHORA_ANSWERS`), the bench's own release profile; the way is `AMPHORA_LOG_READ_INDEX`, read once
per process, so every way ran the same code and calibration (RULES 5e). K0's mixed phase: 1,000
sessions preloaded with 50 events each, then N writers commit events durably while N readers read
sessions' latest 50 events, N = 1, 32 and 128, 6 s with 1 s of warm-up, a fresh store each run. No
checkpoint ran in any mixed phase (`checkpoints` 0), so the log held every page read and every commit
moved the readers' snapshot on: the case the suite's settle hides (lane Q4). kiln needs
cpus=4,mem=12g on adacachy2 (refused at 16: 5 of 32 cores idle).

### Profiles (AMPHORA_PROFILE, 997 Hz), two rounds: the ways in order, then reversed

02:05Z to 02:10Z. Shares of all CPU samples, round 1 / round 2 (`q4_shares.py`, lane Q4's). `lookup`
is every sample under a `find_frame` frame; `lookup spin` the samples ending in `_mm_pause` under it.

find_frame, inclusive:

| clients | locked | filtered | published | optimistic |
|---|---|---|---|---|
| 1 | 8.8 / 9.4 | 9.0 / 9.2 | 7.7 / 10.9 | 5.8 / 5.5 |
| 32 | 14.0 / 17.9 | 15.4 / 18.4 | 14.7 / 15.6 | 0.9 / 0.9 |
| 128 | 16.3 / 24.2 | 20.7 / 28.7 | 33.6 / 29.6 | 1.9 / 1.9 |

find_frame's lock (lookup spin):

| clients | locked | filtered | published | optimistic |
|---|---|---|---|---|
| 1 | 0.0 / 0.0 | 0.0 / 0.0 | 0.2 / 1.9 | 0.0 / 0.0 |
| 32 | 12.5 / 16.4 | 14.3 / 14.8 | 13.0 / 12.6 | 0.1 / 0.1 |
| 128 | 12.7 / 21.1 | 15.7 / 24.4 | 30.1 / 26.4 | 0.2 / 0.1 |

The header (`wal_header`) under `locked`: 6.1 / 12.9 at 32 clients, 3.8 / 2.3 at 128; 0.1-0.3 in the
other three, which read its copy. Samples a cell: 2,199-4,035 at 1 client, 20,651-22,109 at 32,
9,121-14,741 at 128. All spinning was 37-67% at 32 and 128 clients in every way: the rest is the
writers' and the reader pool's waits, which another lane owns.

Why each lost: `filtered` sends every page the log holds to the lock, and a reader beside writers
reads those. `published` copies the whole index under the lock once for each commit a reader sees,
the others reading under the lock meanwhile: at 128 clients it cost the most of the four.

### Reads a second, three rounds, the ways' order rotated each round

02:10Z to 02:16Z. Median of 3 (range), `q5_table.py`. Failures 0, unanswered writes 0, failed runs 0.

| clients | locked | filtered | published | optimistic | filtered / locked | published / locked | optimistic / locked |
|---|---|---|---|---|---|---|---|
| 1 | 7,674 (5,195-11,564) | 9,066 (8,860-14,714) | 8,888 (5,796-9,084) | 6,999 (863-15,570) | 1.18x | 1.16x | 0.91x |
| 32 | 22,437 (21,696-28,396) | 31,989 (31,044-32,041) | 22,344 (21,352-28,180) | 35,962 (33,542-37,475) | 1.43x | 1.00x | 1.60x |
| 128 | 12,698 (11,742-16,680) | 16,570 (13,892-22,038) | 8,312 (6,990-10,954) | 18,403 (13,635-18,708) | 1.30x | 0.65x | 1.45x |

Read p50 / p99 (ms), median of 3: 32 clients `locked` 0.672 / 9.600, `filtered` 0.484 / 7.744,
`published` 0.468 / 11.648, `optimistic` 0.392 / 7.744; 128 clients 2.400 / 98.304, 1.776 / 79.872,
3.168 / 167.936, 1.616 / 75.776. Events written a second, median of 3: 32 clients 1,700, 1,756,
1,760, 2,001; 128 clients 5,211, 4,531, 4,179, 5,415.

At 32 clients `optimistic`'s whole range (33,542-37,475) lies above every other way's. At 128 its
range overlaps `filtered`'s; `published` is below `locked`. At 1 client the ranges overlap: no
measurable difference.

The bound: under `locked` at 32 clients the lookup and the header took 20-31% of the CPU together
(14.0/17.9% and 6.1/12.9%), and `optimistic` removes nearly all of both, so a read can be at most
1/(1 - 0.31) = 1.45x faster on CPU alone. The median ratio, 1.60x, is past it; the ratio of the
ranges' near ends, 33,542 / 28,396 = 1.18x, is inside it. With three rounds and this host's noise
(each way's range 3-30% of its median), the size of the throughput gain is not pinned down; its
direction agrees with the profiles. Against `filtered`, which reads the header's copy too, the
removable share is the lookup's 15-18% (bound 1.22x), and the median ratio is 1.12x.

## Answers

Each way's answers after K0's mixed phase held to SQLite's on the same rows (`q5_answers.sh`,
02:18Z to 02:20Z, binary 105d8482: b6e2e72f with `AMPHORA_ANSWERS` saying how many events each
session was written): one run a way at 1, 32 and 128 clients. After the mixed phase the bench reads
each of the 1,000 sessions' latest 50 events through Amphora's readers, with the log still holding
the pages (no checkpoint ran); SQLite gets exactly the rows the bench wrote (every write answered,
ids `e-0` to `e-<written - 1>`) and answers the same query. 12 runs, 12,000 pages, 50,744-94,936
events a run: 0 differ, in all four ways. Failures 0, unanswered writes 0 in every run.

## Commands

All on adacachy2 (runner 00aad8daff2fa3c9daa65d53c425d450, sync false), scripts in
`2026-11-24-adacachy2-perf-reads-q5/` and lane Q4's `q4_shares.py`, the lane's folder
`~/.cache/analytics-q5`:
- r2: the release bench at 6229da95 and `store::tests::q5_k0_amphora_opens_and_reads_what_it_wrote`
  (cpus=8,mem=24g): the fix, its test passing.
- r14: the bench at b6e2e72f, its test, then `q5_mixed.sh` (cpus=4,mem=12g, after a refusal at 16):
  the profiles and the timed rounds above. Its answer step failed in every run (SQLite can't open
  Amphora's files); that step's numbers are not used.
- r75: the bench at 105d8482 and `WAYS="locked filtered published optimistic" q5_answers.sh`
  (cpus=4,mem=12g), in a second target dir (deleted after, RULES 5c) so it didn't wait behind the
  engine checks' build lock.
- The engine checks of the merged tree, below the deletion: clippy on amphora-engine
  (`--all-targets -D warnings`), `storage::wal`'s tests, `--test log_read_index` and
  scripts/check-production.sh.
