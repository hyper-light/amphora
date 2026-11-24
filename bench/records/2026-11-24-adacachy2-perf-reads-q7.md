# Lane Q7: lane Q6's read lock on the log's record registry, landed and measured (adacachy2, 2026-11-24)

Deliverable status, losses first.

- 128 clients: no measurable difference. Reads B/A 1.23x (14,195 to 17,505 a second, medians of
  10 alternated runs a side, rank-sum p 0.19); writes 0.96x (p 0.85). In the profiled runs at 128 clients, the bench's
  threads waited runnable for a CPU 1.4 to 3.5 times their CPU time (scheduler counters, below).
- 1 client: no measurable difference. Reads 1.04x (p 0.97), writes 1.10x (p 0.44). One client's
  reads ranged from 1,672 to 14,532 a second between runs on this host.
- Writes: no measurable difference at any client count. The change doesn't touch the write path;
  writes wait on the log's flush (6 to 22 ms a flush in 59 of the 60 timed runs).
- The host was busy. cpus=16 was refused (1 of 32 cores idle), so the A/B ran at cpus=4, at load
  45 to 73 on 32 cores and CPU pressure 7% to 51%. The arms alternated, so both saw the same
  load, but every number here is this host's under that load.
- Two profiles a cell, not more, and the first set doesn't profile the mixed phase. Three of its
  four runs flushed slowly (78 to 344 ms a flush), and B's at 32 clients wrote 541 events, a
  read-only phase. The second set (r244) ran with writes flowing (11 to 14 ms a flush, as in the
  timed runs), at load 79 to 89 on 32 cores. The shares below lead with it.
- The profiler drops samples whose PC is in libc, so a thread parked or yielding on the new lock
  is in no profile count. The throughput and the scheduler counters stand for it.
- CHECKS_STATUS
- Done. 32 clients: reads 1.52x (33,366 to 50,691 a second, medians of 10 alternated runs a side,
  ranges 28,066 to 60,755 and 42,377 to 62,895, rank-sum p 0.0007), read p50 0.412 to 0.232 ms,
  p99 7.94 to 4.67 ms, on fewer cores (10.6 to 9.6). In the profiles with writes flowing, the
  registry's share of all samples fell from 44.3% to 3.6% at 32 clients and from 37.9% to 4.8%
  at 128; all spinning from 49.4% to 16.9% and from 39.8% to 11.7%. The next wait is the frame
  cache's lock under `page_frames`: 14.0% of samples at 32 clients, 8.9% at 128. Every timed
  run's answers equal SQLite's: 60 runs, 60,000 pages, none differ.

## What landed

Lane Q6's 235818d1: the shared log's record registry (`WalSharedRuntime::records`,
`packed::Records`) is `crate::sync::RwLock` (parking_lot's), no longer `fast_lock::SpinLock`. Its
read-only lookups (`RecordRead::drive`, `holds_segment_record`, `refuse_record_slot`) take it
shared. Q6's branch (511afe64) merged with main three times in this lane, as main kept moving in
crates/engine: 20a09ee4 (main 9fbfdbaf), 9ddd998e (main 2571f841) and 4d5e13c2 (main c39adf56;
its message says 9ed5b097, as main moved between the fetch and the merge). None of main's changes
touched `storage/wal.rs` or `storage/sqlite3_ondisk.rs`. Main took it as 0c13cf68 at 04:30Z, a
fast-forward from 5bd0f46e, which had moved since c39adf56 only in bench/records.

## Checks

On MacBook Pro 2 (runner e30189ea29182644fe5a4b2e06c7dac2, needs macos/arm64,cpus=8,mem=16g),
`q7_check.sh`, which touches the engine's root first so clippy must re-check it.

- r159, merged tree 20a09ee4: `cargo fmt --all --check` passed. Clippy on amphora-engine
  (`--all-targets --locked -- -D warnings`) passed in 4m44s, and its log shows `Checking
  amphora-engine`. `storage::wal::test::` passed: 110 passed, 1 ignored, Q6's
  `q6_reads_of_the_record_registry_do_not_wait_for_each_other` among them. `packed_log` (16),
  `log_read_index` (1) and `spilled_log` (2, 1 ignored) passed. `scripts/check-production.sh`
  failed in its second pass: `scripts/engine-ratchet.py` imports `tomllib` (Python 3.11) and the
  Mac's `python3` is older. The first pass (every crate but the engine's, the forbidden lints)
  finished.
- r238, merged tree 9ddd998e (main 2571f841), with a python3 that has `tomllib` (3.14.7):
  `cargo fmt --all --check` passed; clippy on amphora-engine passed in 6m44s with `Checking
  amphora-engine` in its log. Stopped there: main had moved inside crates/engine again (lane R10,
  9ed5b097), so its later steps would have checked a stale tree.
- r277, merged tree 4d5e13c2 (main c39adf56), the code main took: `cargo fmt --all --check`
  passed; clippy on amphora-engine passed in 1m44s with `Checking amphora-engine` in its log;
  `storage::wal::test::` 110 passed, 1 ignored, Q6's test among them. That is RULES 5b's gate
  (clippy and the unit tests of what changed), and main took the merge at 04:30Z while the rest
  ran. R277_REST

## The A/B

K0's mixed phase as Q5 and Q6 ran it: 1,000 sessions preloaded with 50 events each, then N writers
commit events durably while N readers read sessions' latest 50 events, N = 1, 32 and 128, 6 s with
1 s of warm-up, a fresh store each run. A = main 9fbfdbaf (my merge's parent), B = 20a09ee4 (the
fix merged with it). Each arm built from its SHA in lane Q6's worktree, one after the other in the
lane's target dir with 4 jobs (2m57s and 2m47s), with the bench workspace's own release profile.
One calibration for both arms (RULES 5e): A's body under B's key, body sha256 b7d4ad7b9a9a8403 in
both. Ten rounds, A,B then B,A, each round every client count; the load and CPU and I/O pressure
were read before each run. Reads a second are every read of the 6 s phase over 6 s, warm-up
included (the bench counts reads that way); events a second are the bench's `write_events_per_s`,
the 5 s after the warm-up.

| clients | arm | runs | reads/s median (range) | events/s median (range) | cores median | read p50 / p99 ms | failures | unanswered |
|---|---|---|---|---|---|---|---|---|
| 1 | A 9fbfdbaf | 10 | 8,654 (1,672-14,532) | 84 (41-139) | 0.7 | 0.079 / 0.704 | 0 | 0 |
| 1 | B 20a09ee4 | 10 | 8,987 (2,800-10,818) | 92 (55-145) | 0.7 | 0.078 / 0.640 | 0 | 0 |
| 32 | A 9fbfdbaf | 10 | 33,366 (28,066-60,755) | 1,484 (146-2,451) | 10.6 | 0.412 / 7.936 | 0 | 0 |
| 32 | B 20a09ee4 | 10 | 50,691 (42,377-62,895) | 1,502 (818-1,907) | 9.6 | 0.232 / 4.672 | 0 | 0 |
| 128 | A 9fbfdbaf | 10 | 14,195 (8,619-19,564) | 4,644 (3,542-6,774) | 6.0 | 1.328 / 102.400 | 0 | 0 |
| 128 | B 20a09ee4 | 10 | 17,505 (12,424-23,718) | 4,443 (3,972-5,745) | 4.6 | 1.160 / 78.848 | 0 | 0 |

B/A and the exact two-sided rank-sum p (`q7_table.py`): 1 client reads 1.04x (p 0.97), events
1.10x (p 0.44); 32 clients reads 1.52x (p 0.0007), events 1.01x (p 0.68); 128 clients reads 1.23x
(p 0.19), events 0.96x (p 0.85).

A's highest 32-client run (round 5, 60,755 reads a second) is the one run whose writes stalled.
It made 36 flushes at 163 ms each, where every other run took 6 to 22 ms a flush, and it wrote 146
events a second. Its readers read with almost no writes beside them. Every other A run at 32
clients read 28,066 to 39,496 a second, every B run 42,377 to 62,895.

Answers: each timed run wrote every session's latest page (`AMPHORA_ANSWERS`) and `q5_answers.py`
held it to SQLite's answer on the same rows, as lane Q5's r75 did. 60 runs, 60,000 pages, 0
differ. No run failed, no write failed or went unanswered.

Every run, in order, with its mixed phase's flushes (stats after the phase less those after the
preload) and the load before it:

| run | reads/s | events/s | cores | mixed flushes | ms a flush | load before |
|---|---|---|---|---|---|---|
| 9fbfdbaf-1-1 | 14,532 | 139 | 1.0 | 846 | 7 | 61.15 50.94 35.60 |
| 20a09ee4-1-1 | 10,818 | 145 | 0.9 | 869 | 6 | 53.88 50.36 35.98 |
| 9fbfdbaf-1-32 | 33,938 | 2,451 | 16.0 | 541 | 7 | 52.20 50.13 36.14 |
| 20a09ee4-1-32 | 62,437 | 1,803 | 12.0 | 405 | 8 | 55.07 50.76 36.41 |
| 9fbfdbaf-1-128 | 13,105 | 6,774 | 7.2 | 463 | 7 | 55.54 50.93 36.55 |
| 20a09ee4-1-128 | 14,177 | 5,745 | 4.5 | 378 | 8 | 56.27 51.23 36.80 |
| 20a09ee4-2-1 | 5,138 | 115 | 0.5 | 609 | 9 | 58.09 51.69 37.03 |
| 9fbfdbaf-2-1 | 8,856 | 109 | 0.8 | 612 | 8 | 55.12 51.38 37.23 |
| 20a09ee4-2-32 | 54,551 | 1,805 | 10.7 | 386 | 8 | 52.92 51.07 37.36 |
| 9fbfdbaf-2-32 | 32,594 | 1,833 | 12.2 | 393 | 9 | 54.89 51.55 37.66 |
| 20a09ee4-2-128 | 19,573 | 4,669 | 4.8 | 312 | 13 | 57.14 52.07 37.91 |
| 9fbfdbaf-2-128 | 19,564 | 5,191 | 7.3 | 394 | 12 | 58.00 52.43 38.18 |
| 9fbfdbaf-3-1 | 9,357 | 67 | 0.8 | 426 | 12 | 58.48 52.62 38.31 |
| 20a09ee4-3-1 | 8,798 | 119 | 0.8 | 732 | 7 | 61.01 53.82 39.10 |
| 9fbfdbaf-3-32 | 28,390 | 1,889 | 11.6 | 392 | 8 | 60.35 54.05 39.41 |
| 20a09ee4-3-32 | 51,533 | 1,907 | 9.6 | 398 | 7 | 59.62 54.13 39.60 |
| 9fbfdbaf-3-128 | 12,515 | 6,072 | 6.4 | 385 | 8 | 61.57 54.62 39.84 |
| 20a09ee4-3-128 | 17,494 | 5,307 | 4.9 | 375 | 10 | 62.28 55.01 40.12 |
| 20a09ee4-4-1 | 9,176 | 84 | 0.7 | 484 | 12 | 64.66 55.63 40.40 |
| 9fbfdbaf-4-1 | 7,565 | 86 | 0.7 | 492 | 11 | 51.85 53.33 40.04 |
| 20a09ee4-4-32 | 48,746 | 1,503 | 9.1 | 301 | 10 | 46.62 52.05 39.97 |
| 9fbfdbaf-4-32 | 32,795 | 1,397 | 10.4 | 323 | 12 | 51.09 52.82 40.36 |
| 20a09ee4-4-128 | 13,929 | 4,217 | 4.1 | 283 | 12 | 55.41 53.69 40.70 |
| 9fbfdbaf-4-128 | 16,270 | 5,422 | 7.3 | 399 | 11 | 58.48 54.41 41.08 |
| 9fbfdbaf-5-1 | 11,947 | 82 | 0.9 | 498 | 11 | 60.12 54.81 41.28 |
| 20a09ee4-5-1 | 6,838 | 79 | 0.7 | 472 | 11 | 57.59 54.58 41.49 |
| 9fbfdbaf-5-32 | 60,755 | 146 | 9.7 | 36 | 163 | 56.68 54.55 41.76 |
| 20a09ee4-5-32 | 54,597 | 943 | 9.7 | 234 | 17 | 67.56 56.93 42.67 |
| 9fbfdbaf-5-128 | 18,406 | 4,216 | 6.5 | 317 | 18 | 73.08 58.49 43.33 |
| 20a09ee4-5-128 | 17,665 | 4,188 | 4.7 | 304 | 13 | 72.19 58.54 43.43 |
| 20a09ee4-6-1 | 6,488 | 63 | 0.6 | 373 | 14 | 44.72 52.57 43.41 |
| 9fbfdbaf-6-1 | 1,672 | 41 | 0.2 | 248 | 18 | 50.09 53.23 43.82 |
| 20a09ee4-6-32 | 62,895 | 818 | 10.3 | 218 | 22 | 55.21 54.12 44.31 |
| 9fbfdbaf-6-32 | 39,496 | 1,250 | 10.5 | 261 | 16 | 56.40 54.39 44.50 |
| 20a09ee4-6-128 | 17,830 | 3,972 | 4.5 | 294 | 15 | 57.69 54.74 44.72 |
| 9fbfdbaf-6-128 | 13,838 | 3,945 | 5.0 | 297 | 15 | 57.48 54.74 44.77 |
| 9fbfdbaf-7-1 | 6,138 | 49 | 0.6 | 296 | 18 | 57.35 54.80 44.90 |
| 20a09ee4-7-1 | 10,050 | 55 | 0.8 | 343 | 16 | 53.66 54.13 44.94 |
| 9fbfdbaf-7-32 | 36,291 | 1,295 | 9.5 | 251 | 14 | 50.76 53.36 44.93 |
| 20a09ee4-7-32 | 48,438 | 1,082 | 8.2 | 221 | 17 | 55.11 54.22 45.25 |
| 9fbfdbaf-7-128 | 17,231 | 3,542 | 5.5 | 295 | 18 | 57.17 54.69 45.50 |
| 20a09ee4-7-128 | 23,718 | 4,174 | 5.8 | 326 | 15 | 57.08 54.71 45.56 |
| 20a09ee4-8-1 | 2,800 | 82 | 0.4 | 489 | 10 | 56.35 54.61 45.62 |
| 9fbfdbaf-8-1 | 8,946 | 79 | 0.7 | 465 | 12 | 52.50 53.87 45.57 |
| 20a09ee4-8-32 | 49,849 | 1,448 | 9.5 | 302 | 11 | 49.23 53.06 45.48 |
| 9fbfdbaf-8-32 | 35,174 | 1,570 | 11.7 | 347 | 11 | 52.86 53.71 45.77 |
| 20a09ee4-8-128 | 17,516 | 4,832 | 4.9 | 339 | 11 | 55.03 54.15 45.96 |
| 9fbfdbaf-8-128 | 14,553 | 4,437 | 5.5 | 329 | 13 | 59.26 55.07 46.35 |
| 9fbfdbaf-9-1 | 8,453 | 99 | 0.8 | 561 | 10 | 61.08 55.52 46.54 |
| 20a09ee4-9-1 | 9,582 | 101 | 0.9 | 569 | 9 | 54.70 54.44 46.38 |
| 9fbfdbaf-9-32 | 28,066 | 1,869 | 10.7 | 367 | 9 | 58.37 55.24 46.81 |
| 20a09ee4-9-32 | 44,885 | 1,528 | 7.9 | 315 | 10 | 60.74 55.78 47.03 |
| 9fbfdbaf-9-128 | 12,860 | 4,850 | 5.1 | 330 | 13 | 63.64 56.56 47.38 |
| 20a09ee4-9-128 | 14,418 | 4,848 | 4.0 | 303 | 10 | 63.91 56.74 47.48 |
| 20a09ee4-10-1 | 9,850 | 121 | 0.8 | 720 | 8 | 63.46 56.88 47.63 |
| 9fbfdbaf-10-1 | 6,538 | 88 | 0.6 | 514 | 11 | 56.74 55.72 47.40 |
| 20a09ee4-10-32 | 42,377 | 1,502 | 7.7 | 314 | 9 | 53.24 54.98 47.33 |
| 9fbfdbaf-10-32 | 28,880 | 1,344 | 8.6 | 263 | 13 | 59.76 56.30 47.84 |
| 20a09ee4-10-128 | 12,424 | 4,217 | 3.5 | 246 | 11 | 65.26 57.61 48.36 |
| 9fbfdbaf-10-128 | 8,619 | 3,610 | 3.5 | 214 | 12 | 64.04 57.48 48.37 |

## Profiles: spin and the registry

Two profiles an arm a client count (AMPHORA_PROFILE, 997 Hz, the mixed phase): the first before
the timed rounds (r161), the second after them (r244). Spin is a sample whose leaf is `_mm_pause` or `spin_loop` (Q4's and Q6's
definition). The registry is a sample with its lock's or a guard's frame (`lock<Records>`,
`read<RawRwLock, Records>`, a guard's drop) or one of its methods (q6_hold.py's set) on the stack
(`q7_limit.py`); "any frame" counts every frame naming `packed::Records`, its `Arc` included
(`q7_leaf.py`). The frame cache's lock is Q6's row: spin under `page_frames`.

| | A, 32 | B, 32 | A, 128 | B, 128 |
|---|---|---|---|---|
| **second profile (r244)**: ms a flush in the mixed phase | 11 | 14 | 12 | 11 |
| events written; reads a second | 7,704; 29,825 | 4,800; 40,876 | 22,185; 10,830 | 24,798; 15,372 |
| samples | 13,339 | 10,147 | 8,409 | 7,465 |
| all spinning | 49.43% | 16.91% | 39.83% | 11.71% |
| the registry: its lock, guards and methods | 44.25% | 3.57% | 37.92% | 4.82% |
| the registry: any frame naming it | 42.67% | 2.11% | 35.55% | 1.71% |
| the frame cache's lock, spin under `page_frames` | 7.47% | 13.98% | 4.98% | 8.94% |
| the change log's lock, spin under `changed_pages` | 1.66% | 2.75% | 1.82% | 1.62% |
| **first profile (r161)**: ms a flush in the mixed phase | 4 | 344 | 78 | 144 |
| events written; reads a second | 15,578; 29,660 | 541; 101,764 | 5,754; 46,308 | 2,949; 39,900 |
| samples | 19,740 | 13,900 | 8,616 | 7,249 |
| all spinning | 44.41% | 1.58% | 9.30% | 2.90% |
| the registry: its lock, guards and methods | 56.07% | 1.98% | 10.84% | 1.78% |
| the registry: any frame naming it | 55.84% | 2.41% | 10.49% | 1.17% |
| the frame cache's lock, spin under `page_frames` | 6.34% | 1.18% | 1.49% | 2.01% |
| the change log's lock, spin under `changed_pages` | 1.48% | 0.29% | 0.19% | 0.59% |

On A the registry is the largest spin in both sets (q6_split.py's rows, second set: 28.76% under
`holds_segment_record` and 11.29% under `drive` at 32 clients; 20.42% and 10.85% at 128), as Q6
measured on main 111f0974 (34.4% and 39.1%). A's first 128-client profile spun 9.3%: that run wrote
5,754 events against 22,185 in the second, and its readers read 46,308 a second against 10,830.

The scheduler counters beside the second profiles: the bench's threads waited runnable for a CPU
1.95x (A) and 2.68x (B) their CPU time at 32 clients, 3.53x and 3.39x at 128.

The scheduler counters of the bench's threads beside each first profile (`q6_sched.py`):

| | A, 32 | B, 32 | A, 128 | B, 128 |
|---|---|---|---|---|
| bench threads | 74 | 74 | 202 | 202 |
| their CPU, s | 94.94 | 86.30 | 40.76 | 30.56 |
| runnable but waiting for a CPU, s | 83.95 | 85.23 | 58.02 | 62.94 |
| waiting / CPU | 0.88 | 0.99 | 1.42 | 2.06 |
| voluntary switches | 39,144 | 54,225 | 852,537 | 658,787 |
| involuntary switches | 379,153 | 393,623 | 253,245 | 197,682 |

## What limits the mixed phase now

From B's second profile (writes flowing), largest first. None of it is changed here.

- The frame cache's lock (a `SpinLock<HashMap>`), spun on under `page_frames`: 13.98% of all
  samples at 32 clients, 8.94% at 128. It is now the largest single wait. Q6 measured it at 4.0% and 2.1% behind the registry; A's second profile has it at
  7.47% and 4.98%. In the first set, with writes slowed or stalled, it was 1.18% and 2.01% on B.
- Reading pages from the log, on CPU: `read_page_no_cache` is 43.5% of all samples at 32 clients
  and 47.6% at 128 (by total). Within it, after the frame cache: the change log's lock under
  `changed_pages` (2.75%, 1.62%), the page cache's write-guard release in `cache_insert` (its
  compare-and-swap 2.79%, 2.02%), `evict_one` (its `is_dirty` loads 1.70%, 1.45%; the dropped
  `Page` and `Buffer` refcounts 2.61%, 2.67%), and `Arc` refcount traffic on the log's
  `SharedLog`, `File`, `BufferPool` and `Record` objects (each 0.6% to 1.5%).
- The registry: 3.57% and 4.82% (lock, guards and methods), 2.11% and 1.71% by any frame naming it.
- The host: at 32 clients the bench's threads waited runnable for a CPU 0.99x their CPU time in
  the first profile and 2.68x in the second; at 128 clients 2.06x and 3.39x. On this host at this
  load, B's threads spent about as long or longer waiting for a CPU as running.

The profiler can't see a thread parked on a lock (its samples in libc are dropped), so waits on
the parking locks (the registry now, the page cache's `RwLock`, the segment's `RwLock`) are not in
these shares.

## Commands

All on adacachy2 (runner 00aad8daff2fa3c9daa65d53c425d450, sync false, needs
linux/amd64,cpus=4,mem=12g after cpus=16 was refused), scripts beside this record, run from
lane Q6's GitHub clone there (`~/.cache/amphora-perf/analytics-remote`):
- r161: `A=9fbfdbaf B=20a09ee4 TOOLSHA=20a09ee4 JOBS=4 DEADLINE=<04:22Z> q7_ab.sh`: both arms
  built, profiled at 32 and 128 (`q6_split.py`, `q7_limit.py`), calibration shared, rounds 1-5.
- r205: `FROM=6 TO=10 q7_more.sh`: `q7_leaf.py` on r161's profiles, rounds 6-10, `q7_table.py`
  over all ten.
- r244: `q7_prof2.sh`: a second profile of each cell.
- Checks: r159, r238 and r277 on MacBook Pro 2, `q7_check.sh`.
