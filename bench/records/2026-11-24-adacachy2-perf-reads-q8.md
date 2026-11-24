# Lane Q8: the frame cache's wait under `page_frames`, named and fixed at its cause (adacachy2, 2026-11-24)

The lane's hour: 04:59:28Z to 05:59:28Z. Main before this lane's change: 2a4ccaa4. Fix: c7fc03f4
(branch perf/analytics), merged with main 3a58a13f as 4b9108d4.

Deliverable status, losses first.

- 32 and 128 clients: no measurable difference in reads or writes a second, the A/B's 4 alternated runs a
  side (2a4ccaa4 against c7fc03f4, loads 48 to 66 on 32 cores; answers: 24 runs, 24,000 pages, 0 differ). 32 clients: reads 0.94x
  (57,314 to 53,770 a second, ranges 50,112 to 66,647 and 49,356 to 74,087, rank-sum p 0.69), events 1.09x
  (p 0.89). 128 clients: reads 1.03x (19,567 to 20,195, p 0.89), events 0.90x (p 0.69). The lock's wait is
  gone, and the cores the bench used fell 10.5 to 9.1 at 32 clients and 5.6 to 4.9 at 128 (medians of 4
  runs, their spread not tested), but reads a second don't follow: something else limits this host (below).
- 1 client: reads 1.26x (10,484 to 13,200 a second, ranges 7,452 to 12,484 and 12,080 to 15,368, p 0.11, so
  not significant on 4 runs a side); writes 0.85x (p 0.69).
- The host was busy for the whole lane (load 40 to 66): cpus=16 was refused, then 4, then 2. The A/B ran at needs cpus=1
  (the bench itself uses 5 to 11 cores), B's build at 3 jobs. Arms alternated, so both saw the same load.
- Attribution rests on one profile a client count and one run a client count of the timed-lock diagnostic
  build (branch perf/analytics-q8-diag, not for main; the timer adds about two clock reads a lock
  taking), each a single run on that host. `page_frames`' 36,000 waits past 65 us are nearly all of
  its 16.4 s, as the 2^17 ns and larger buckets sum (the bucket's ends only bound each wait).
- The profiler drops samples whose PC is in libc, so a holder parked or preempted shows only in the diagnostic
  build, not in the profile; after the fix, its waiters park (a `RwLock`) and are in no row either.

## What the profile shows (main 2a4ccaa4, r313)

One profile of K0's mixed phase at each client count, as Q7's `prof()` took it (AMPHORA_PROFILE at
997 Hz, 1,000 sessions preloaded with 50 events each, N writers and N readers for 6 s), the load before
and after each run. The bench's threads waited runnable for a CPU 1.35 times their CPU time at 32
clients and 3.38 times at 128. (`q8_frames.py` gives the rows below; it counts every sample with the
frame cache's lock's frame on its stack, whatever the leaf.)

| | 32 clients | 128 clients |
|---|---|---|
| load before (1, 5, 15 min), CPU pressure | 48.5 31.4 27.8, 20.4% | 50.3 32.3 28.1, 37.4% |
| reads/s; events/s; cores | 56,306; 1,679; 10.9 | 15,801; 6,267; 4.3 |
| samples | 12,199 | 7,905 |
| at the frame cache's lock: waiting | 11.49% | 14.03% |
| of which leaf `_mm_pause`/`spin_loop` | 10.65% | 13.41% |
| by taker: `page_frames` | 11.33% | 13.14% |
| `find_frame_locked`; `latest_frame` | 0.16%; 0.00% | 0.67%; 0.22% |
| in a taker and not waiting (on CPU) | 1.2% | 0.6% |

**Callers.** Every `page_frames` sample (11.33% and 13.14%) is one chain: `page_frames` <
`begin_read_record` < `read_frame` < `read_page_no_cache` < `read_page_uncached` (reader threads,
`amphora-bench-#`). Reading a page out of a packed record asks for the page's last frames, once a
page: the caller is the closure that builds the read's `PageReader`s.

**Holders, on CPU.** They hold it for work that rarely shows: the samples in a taker that isn't
waiting are 1.2% at 32 clients and 0.6% at 128 (`page_frames`: hash lookup, binary search, copying the
frame list, 0.6% at 32 clients in the read guard's drop; `cache_frame` and `cache_record`: 0.1% in
all). The profiler drops samples whose PC is in libc, so a holder that is parked or preempted, and an
allocation, are in no row. Waiting is 11.5 and 14.0%, on-CPU holding 1.2 and 0.6%.

**What each holder does under the lock, and why (by reading `crates/engine/storage/wal.rs`).**
- `page_frames` (readers): finds the page's list (a hash lookup), its last frames up to the read's head
  (a binary search), and copies at most 256 of them (`packed::MOST_DELTAS` + 1) into a new vector. It
  needs the list's contents at one moment, so it holds the lock for the copy.
- `latest_frame` and `find_frame_locked` (`find_frame` when the lock-free index can't answer): the same
  hash lookup and binary search, no copy.
- `cache_frame` (the writer, for each frame of a commit): it drops what a rewind overwrote, records the
  page's new frame in the map, tells the lock-free index (`ReadIndex::appended`), the readers' change
  list (`ChangeLog::record`), and the high-water. These must agree with each other and with the frame
  lists, so it holds the lock for all of them.
- `cache_record` (the writer, for each packed record): as `cache_frame`, for a record: it first publishes
  the record's slots in the record registry (`records.write()`), a read-write lock whose write waits
  for the readers that hold it shared. It held the frame cache while it waited: since a page may only
  map to the record once the record is known, it took the registry after the frame cache, inside the
  frame cache's lock.
- `rollback_cache` (a transaction's rollback), recovery, checkpointing (`scan_latest_frames`: a walk of
  the whole map) and the build of the lock-free index (one a generation): not in these profiles.

## Why a reader waits (a lock timed at each call site, r348, 8df232ef on branch perf/analytics-q8-diag)

The profile can't see a holder that is off a CPU. So the frame cache's lock was timed in a diagnostic
build (not for main: `fast_lock::diag::DiagLock`, each `lock()` call site's waits and holds and for
each wait past 10 us the site that had last taken the lock), run at 32 and 128 clients for the same 6 s
with loads of 31 to 34 (other lanes' builds, 4 runnable-thread kiln test suites in the host's load),
no profiler.

| | 32 clients | 128 clients |
|---|---|---|
| reads/s; events/s; cores | 57,428; 3,758; 15.5 | 26,623; 10,939; 11.3 |
| `page_frames`: acquisitions | 841,036 | 590,299 |
| `page_frames`: waited in all, mean, longest | 16.42 s, 19.5 us, 4.27 ms | 7.59 s, 12.9 us, 5.45 ms |
| `page_frames`: held, mean, longest | 600 ns, 3.08 ms | 881 ns, 2.23 ms |
| `cache_record` (the writer): acquisitions | 7,375 | 14,358 |
| `cache_record`: held in all, mean, longest | 723 ms, 98 us, 4.16 ms | 464 ms, 32 us, 4.16 ms |
| of it, waiting for `records.write()` | 314.7 ms, 43 us mean, 3.63 ms longest | 200.3 ms, 14 us, 4.16 ms longest |
| `cache_record`: inserts into the map; `ReadIndex::appended`; the change list's lock | 6.1; 10.2; 4.3 ms | 8.8; 14.8; 5.4 ms |
| `cache_frame`: acquisitions, held mean, longest | 31,108, 1.35 us, 1.82 ms | 81,536, 0.93 us, 4.22 ms |
| `latest_frame`: acquisitions, held mean | 58,449, 805 ns | 125,035, 637 ns |
| `find_frame_locked`: waited in all | 197 ms | 342 ms |

Who a `page_frames` wait past 10 us ended behind (the last taker when the wait began), in all:

| | 32 clients | 128 clients |
|---|---|---|
| `cache_record` | 21,198 waits, 9.00 s | 10,230 waits, 3.60 s |
| another `page_frames` | 20,848 waits, 6.10 s | 11,431 waits, 3.38 s |
| `latest_frame` | 1,832 waits, 0.68 s | 520 waits, 0.23 s |
| `cache_frame` | 977 waits, 0.39 s | (80) |
| `find_frame_locked` | 59 waits, 8 ms | 71 waits, 15 ms |

The waits are not many short ones but few long ones: of `page_frames`' 841,036 calls at 32 clients,
36,000 waited longer than 65 us (the buckets 2^17 ns and up), about 98% of the 16.4 s.

**Cause.** The frame cache is a lock every reader of the log takes alone, and two of its holders are
long:
1. the writer's `cache_record` waits for the record registry's write lock while it holds the frame cache:
   98 us of holding on average and 4.2 ms at most (43 us and 3.6 ms of it waiting), which is what 55%
   of the long waits were behind; and
2. a reader taken off its CPU in `page_frames` (the host had 1.35 and 3.38 runnable threads waiting
   per running thread; the longest holds are 3.1 and 2.2 ms) holds up every other reader, since
   readers take the lock one at a time: 37% of the long waits.
The record registry's read lock (Q6) is what made the first longer than a writer's CPU time: the writer's
`records.write()` waits for the readers inside the registry's shared lock, one of which may be off its
CPU, while it holds the frame cache, which `page_frames` takes alone.

## The fix (c7fc03f4)

- The frame cache (`WalSharedRuntime::frame_cache`, and recovery's and the tests') is `crate::sync::RwLock`
  (parking_lot's), no longer `fast_lock::SpinLock`: its read-only lookups (`page_frames`, `latest_frame`,
  `find_frame`'s locked path and the index it builds, `scan_latest_frames`, the authority's sync) take
  it shared. A reader taken off its CPU while it holds it holds up no other reader.
- No writer waits for the record registry while it holds the frame cache: every place that takes both takes
  the registry first (`cache_record`, `cache_frame`'s rewind, `rollback_cache`, recovery's
  `flush_pending_frames` and `finalize_loading`). `cache_frame` takes the registry first only for a
  rewind (a slot at or below the cache's high-water), which it looks for before and after taking the
  cache's lock.
- Test: `q8_reads_of_the_frame_cache_do_not_wait_for_each_other`: while a reader holds the frame cache,
  a second finds a page's frames and its latest frame, and once the first lets go a writer's append is in
  the next answer. Its wait is a hang guard, not a measure. It fails by not finishing its read on the
  spin lock (no shared take).

## The A/B (r385)

As Q7's `q7_ab.sh` (`q8_ab.sh`: K0's mixed phase, 1,000 sessions, 50 events preloaded, N writers and N
readers, N = 1, 32, 128, 6 s with 1 s warm-up; answers held to SQLite's by `q5_answers.py`), A = main
2a4ccaa4, B = the fix c7fc03f4 (the same base). A's binary was already built (r313's). One calibration for
both arms (RULES 5e): A's body under B's key, body sha256 976d1df684e7de79 in both. 4 rounds, A,B then B,A.

| clients | arm | runs | reads/s median (range) | events/s median (range) | cores median | read p50 / p99 ms | failures | unanswered |
|---|---|---|---|---|---|---|---|---|
| 1 | A 2a4ccaa4 | 4 | 10,484 (7,452-12,484) | 101 (78-141) | 0.9 | 0.076 / 0.472 | 0 | 0 |
| 1 | B c7fc03f4 | 4 | 13,200 (12,080-15,368) | 85 (39-115) | 1.0 | 0.066 / 0.197 | 0 | 0 |
| 32 | A 2a4ccaa4 | 4 | 57,314 (50,112-66,647) | 1,510 (1,040-1,799) | 10.5 | 0.197 / 4.320 | 0 | 0 |
| 32 | B c7fc03f4 | 4 | 53,770 (49,356-74,087) | 1,650 (939-1,866) | 9.1 | 0.229 / 4.192 | 0 | 0 |
| 128 | A 2a4ccaa4 | 4 | 19,567 (15,996-20,717) | 5,498 (3,940-7,076) | 5.6 | 1.208 / 66.560 | 0 | 0 |
| 128 | B c7fc03f4 | 4 | 20,195 (17,550-24,542) | 4,964 (4,507-5,809) | 4.9 | 0.960 / 67.584 | 0 | 0 |

B/A and the exact two-sided rank-sum p: 1 client reads 1.26x (p 0.114), events 0.85x (p 0.686); 32
clients reads 0.94x (p 0.686), events 1.09x (p 0.886); 128 clients reads 1.03x (p 0.886), events 0.90x
(p 0.686). 24 runs, 24,000 pages held to SQLite's: 0 differ.

Profiles of both arms (one a client count each, at loads 55 to 57; `q8_frames.py`, `q7_limit.py`):

| | A, 32 | B, 32 | A, 128 | B, 128 |
|---|---|---|---|---|
| samples | 13,649 | 13,601 | 9,015 | 7,698 |
| at the frame cache's lock | 11.62% | 0.00% | 6.27% | 0.00% |
| all spinning | 13.38% | 2.12% | 7.52% | 2.38% |
| record registry | 5.24% | 4.93% | 5.82% | 4.88% |

## What limits the mixed phase now

From B's profiles, largest first (none of it changed here): the change log's lock, spun under
`changed_pages` (2.01% of samples at 32 clients, 2.25% at 128), the largest spin left; the record
registry (4.9%, 2.8% of it waiting at 32 clients, 1.1% at 128: its lock's waiters, some in `drive`); and the
host: the bench's threads waited runnable for a CPU 1.10 times their CPU time at 32 clients and 3.0 at
128 (A: 1.18 and 1.72), on a machine whose load was 55. Reads a second did not rise with the spin gone:
at 32 clients the readers' wait is elsewhere, and the profile does not name it (it has no off-CPU rows).
The CPU that the spin burned went (cores 10.5 to 9.1). What the bench is limited by at these loads
was not measured.

## Checks

- The fix before the merge (c7fc03f4, on MacBook Pro 2, runner e30189ea29182644fe5a4b2e06c7dac2, r384,
  `q7_check.sh` from the synced clone): clippy on amphora-engine (`--all-targets --locked -- -D
  warnings`) exit 0; `storage::wal::test::` 111 passed, 0 failed, 1 ignored, `q8_reads_of_the_frame_cache_do_not_wait_for_each_other`
  and Q6's among them; `packed_log` 16, `log_read_index` 1, `spilled_log` 2 passed (1 ignored).
  clippy's log shows `Compiling amphora-engine` (not `Checking`: my script counted 0 of the latter
  and ended 3; RULES 6 counts either).
- The merged trees, with main's later commits (main moved in crates/engine three times in this hour, PG14/PG15,
  F6's ratchet and T21's optimizer; also in crates/amphora, crates/api, CLAUDE.md, vendored hyper-raft and scripts):
  - 87d37672 (main 1f4a8e88): Mac, r424 (`STEPS='clippy wal'`): clippy on amphora-engine exit 0, its log `Checking
    amphora-engine`; `storage::wal::test::` 111 passed, 0 failed, 1 ignored. `cargo fmt --all --check` clean
    before each commit. adacachy2, Linux: r425, `scripts/check-production.sh`, exit 0, `engine-ratchet: 10903
    left ... as the ratchet holds`, its log `Checking amphora-engine`.
  - d1718017 (main 15dd57d8, F6's ratchet): Mac, r456 (`STEPS='clippy wal'`): clippy exit 0,
    `Checking amphora-engine` in its log; `storage::wal::test::` 111 passed, 0 failed, 1 ignored. adacachy2, Linux: r454, check-production
    exit 0, `engine-ratchet: 10617 left ... as the ratchet holds`, its log `Checking amphora-engine` (line 31).
  - 4b9108d4 (main 3a58a13f), check-production on adacachy2 (r391): exit 0, 10903 left.
  - a3f712c8 (main ac2c429a: V18's replicated.rs in crates/amphora, T21's optimizer/lip.rs in crates/engine, 10 lines), re-run
    because main moved in crates/engine: Mac, r464 (`STEPS='clippy wal'` with CARGO_BUILD_JOBS 8, needs cpus=6): clippy on amphora-engine exit
    0, its log `Checking amphora-engine`, `storage::wal::test::` 111 passed, 0 failed, 1 ignored (the same 111 on
    87d37672 and d1718017); adacachy2 (r465): check-production exit 0, 10617 left, its log `Compiling
    amphora-engine` (RULES 6 counts Compiling too). The commit that main takes is that tree with its
    message amended (`git diff a3f712c8 HEAD` empty).

## Commands

All on adacachy2 (runner 00aad8daff2fa3c9daa65d53c425d450) with sync false, from `~/.cache/amphora-perf/analytics-remote`,
builds in `~/.cache/amphora-perf-target/analytics` (lane Q6's worktree `~/.cache/analytics-q6/src`),
except the Mac's, synced from the lane's clone.
- r313 (needs linux/amd64,cpus=4,mem=12g, JOBS=4): `q8_prof.sh` with ARMS=2a4ccaa4...
- r348 (needs cpus=1,mem=8g, 2 jobs): the diagnostic build of 8df232ef and its runs (AMPHORA_FRAME_DIAG), inline.
- r385 (needs cpus=1,mem=8g, 3 jobs): `q8_ab.sh` A=2a4ccaa4 B=c7fc03f4 TOOLSHA=c7fc03f4 ROUNDS=4.
- r391, r425 (needs cpus=1; cpus=2, mem=8g): check-production, as above.
- r384, r424: the Mac checks.
- cpus=16, 4 and 2 were refused during the lane (the host showed 1 or 2 idle cores).
