# Lane Q6: K0's mixed phase's spin attributed; the log's record registry read under a read lock (adacachy2, 2026-11-24)

Deliverable status, losses first.

- Not done: the profile after the fix, the reads and writes a second alternated with main, and the
  answers held to SQLite's. adacachy2 was the verifier's from 03:00Z, past this lane's hour. The
  fix merged once clippy and its tests passed (RULES 5b).
- One profile a cell, not two: each at 32 and 128 clients, on main and on Q5's binary.
- The held-by-a-preempted-holder share is inferred from scheduler counters, not measured per
  acquisition (below).
- Q5's reading was wrong. Q5 called the spin outside the frame lookup "the writers' and the
  reader pool's waits". Attributed by call chain, it is the readers' record registry: 86% of all
  spinning at 32 clients and 93% at 128. The writers' waits are 0.1-0.5% of samples, the reader
  pool's checkout under 0.1%.
- Fixed: the registry (`WalSharedRuntime::records`, `packed::Records`) is the engine's
  `sync::RwLock` (parking_lot's), no longer `fast_lock::SpinLock`. Its read-only lookups take it
  shared; what changes it takes it exclusive. Test:
  `storage::wal::tests::q6_reads_of_the_record_registry_do_not_wait_for_each_other`. Decision:
  `docs/design/store.md`, "Decision (lane perf-Q6)".

## What was measured

K0's mixed phase as Q5 ran it: 1,000 sessions preloaded with 50 events each, then N writers commit
events durably while N readers read sessions' latest 50 events, N = 32 and 128, 6 s with 1 s of
warm-up, a fresh store each run, AMPHORA_PROFILE at 997 Hz. Two binaries on adacachy2 (32 cores):
main 111f0974 (built in the lane's target dir, 3m36s) and Q5's b6e2e72f with
`AMPHORA_LOG_READ_INDEX=optimistic` (the way main kept). Beside each run, every bench thread's
scheduler counters, read every 0.2 s from `/proc/<pid>/task/*/{schedstat,status}` (`q6_sched.py`).

| run | binary | when | load (1/5/15 min) before, after | CPU pressure avg10 |
|---|---|---|---|---|
| r52 32 | b6e2e72f | 02:45:20Z | 46.63 52.02 45.71, 49.46 52.52 45.91 | 30.0%, 46.6% |
| r52 128 | b6e2e72f | 02:45:28Z | 49.46 52.52 45.91, 52.39 53.04 46.15 | 46.6%, 42.3% |
| r59 32 | main 111f0974 | 02:51:34Z | 28.57 41.50 43.67, 32.06 41.80 43.74 | 9.5%, 28.8% |
| r59 128 | main 111f0974 | 02:51:41Z | 32.06 41.80 43.74, 35.26 42.30 43.89 | 28.8%, 35.0% |

Other work on the host at r52's start (`ps`, %CPU): a kiln runner 640 (4,197 threads), three
Amphora test binaries 527, 351 and 292, two mantle processes 95 and 69, rustc 86, clippy 72.

### Every spin sample by its lock and caller, main 111f0974

`q6_split.py`: a spin sample is one whose leaf is `_mm_pause` or `spin_loop` (Q4's definition);
its lock is the frame above the spin, its caller the next frame up. The rows sum to the total.

| lock (thread) | caller | 32 clients | 128 clients |
|---|---|---|---|
| `SpinLock<Records>` (readers) | `holds_segment_record` | 17.75% | 15.54% |
| `SpinLock<Records>` (readers) | `RecordRead::drive` | 16.67% | 23.06% |
| `SpinLock<Records>` (writer) | `holds_segment_record` | under 0.05% | 0.43% |
| `SpinLock<Records>` (readers) | `RecordRead::arrived` | under 0.05% | 0.08% |
| `SpinLock<HashMap>`, the frame cache (readers) | `page_frames` | 4.03% | 2.10% |
| `SpinLock<HashMap>`, the frame cache (readers) | `find_frame_locked` | under 0.05% | 0.17% |
| `SpinLock<ChangeLog>` (readers) | `changed_pages` | 1.17% | 0.72% |
| no frame above the spin (readers) | | under 0.05% | 0.10% |
| the rest: 10 and 4 rows under 0.05% each, the reader pool's checkout among them | | 0.21% | 0.05% |
| all spinning | | 39.82% (8,995 of 22,587) | 42.25% (6,188 of 14,646) |
| of it the record registry | | 34.42% | 39.11% |

The raw chains (`q6_raw.py`, main at 32 clients): 15.4% of all samples are
`lock<packed::Records>` < `holds_segment_record` < `holds_record` < `WalFile::read_frame` <
`read_page_no_cache` < ... < `tablebtree_move_to`; 11.7% `lock<packed::Records>` < `drive` <
`begin_read_record` < `read_frame` < ... < `read_header_page` < `with_header` (the schema cookie's
read at each statement); 2.1% the same through `indexbtree_move_to`. Every page a reader reads of a
packed or mixed generation takes the registry twice: once to ask whether the frame holds a record
(`holds_segment_record`, `slots_of`), once for its chain (`drive`, `slots_of` and `decoded`).
Those lookups take `&self`; only `publish`, `keep`, `drop_after` and `clear` change it.

Q5's binary, same split (`q6_hold.py`): 32 clients spin 54.9%, the registry 47.3%, the frame
cache 5.9%; 128 clients 47.1%, 41.9%, 3.7%.

### Does a preempted lock holder explain any of it?

Yes, part of it, by inference from the scheduler counters. The rest is readers queued behind each
other on one exclusive lock.

| | main, 32 | main, 128 | Q5's, 32 | Q5's, 128 |
|---|---|---|---|---|
| bench threads | 74 | 202 | 74 | 202 |
| their CPU, s | 103.28 | 61.98 | 79.15 | 49.95 |
| runnable but waiting for a CPU, s | 74.76 | 86.73 | 96.35 | 93.19 |
| waiting / CPU | 0.72 | 1.40 | 1.22 | 1.87 |
| timeslices | 589,110 | 1,107,141 | 486,480 | 922,178 |
| voluntary switches | 49,133 | 652,060 | 35,516 | 534,648 |
| involuntary switches | 539,979 | 455,418 | 450,981 | 387,753 |
| mixed phase CPU (bench's `mixed_cpu_s`) / 6 s, cores | 17.8 | 10.7 | 13.5 | 8.7 |
| samples in the registry's critical sections, running | 1.24% | 2.40% | 2.33% | 2.44% |

The arithmetic, main:
- Held by a running holder: 1.24% x 17.8 cores = 0.22 of wall time at 32 clients; 2.40% x 10.7 =
  0.26 at 128.
- Spinning on it: 34.42% x 17.8 = 6.1 cores at 32; 39.11% x 10.7 = 4.2 cores at 128.
- Held by a holder off its CPU (inferred): involuntary switches a second in the 7 s of warm-up and
  phase, times the registry's share of the threads' CPU (where a preemption lands holding it),
  times the mean runnable wait a timeslice. 32: 77,140/s x 1.24% x 127 us = 0.12 of wall time.
  128: 65,060/s x 2.40% x 78 us = 0.12.

So about a third of the registry's held time was a holder taken off its CPU, at both client
counts. `SpinLock` never yields, so every reader that reached a page read spun through it. The
other two thirds are running holders with 18 to 28 cores' worth of spinners queued per unit of
hold: the lock was exclusive for lookups that only read. A parking reader-writer lock removes
both: readers no longer wait on each other's lookups, and a waiter behind a writer parks.

Limits of this attribution:
- pprof's process timer caught about a fifth of the CPU at 32 clients (22,587 samples for 107
  cpu-s at 997 Hz: signals coalesce). Shares assume the loss falls evenly across threads.
- A waiter's swap in `SpinLock::lock` (leaf not a pause) counts as neither spin nor held, so the
  registry's wait is undercounted.
- The preempted share rests on uniform preemption over a thread's CPU time and the mean wait a
  timeslice applying to holders. Hold times measured per acquisition would settle it.

## The fix

`crates/engine/storage/wal.rs` and `sqlite3_ondisk.rs`: `Arc<SpinLock<Records>>` becomes
`Arc<RwLock<Records>>` (`crate::sync::RwLock`: parking_lot's, shuttle's under `cfg(shuttle)`).
`.read()` at the three read-only sites (`RecordRead::drive`, `holds_segment_record`,
`refuse_record_slot`), `.write()` at the rest. No other lock changed; no read is ever nested in
another on the same thread, so parking_lot's writer fairness can't deadlock a reader.

## Checks

On MacBook Pro 2 (runner e30189ea29182644fe5a4b2e06c7dac2, needs macos/arm64,cpus=8,mem=16g;
adacachy2 was the verifier's): clippy on amphora-engine (`--all-targets --locked -D warnings`),
`storage::wal::tests::`, `--test packed_log --test log_read_index --test spilled_log`, and
`scripts/check-production.sh`. Results in the merge commit.

## Commands

All on adacachy2 (runner 00aad8daff2fa3c9daa65d53c425d450, sync false), scripts in
`2026-11-24-adacachy2-perf-reads-q6/`, the lane's folder `~/.cache/analytics-q6`:
- r52 (cpus=4,mem=12g): `SHA=111f0974... DEADLINE=<02:59:30Z> q6_profile.sh`: Q5's binary
  profiled at 32 and 128 with `q6_sched.py` beside each run, then `q6_spins.py`. Its build of main
  failed at once: the bench workspace keeps no lock file, so `--locked` refuses to make one.
- r59 (cpus=2,mem=8g, refused at 4: 2 of 32 cores idle): `q6_main.sh`: `q6_raw.py` on r52's
  profiles, main built in `~/.cache/amphora-perf-target/analytics` (2 jobs, offline, 3m36s), main
  profiled at 32 and 128, then `q6_spins.py` and `q6_raw.py`.
- Two cpus=1 runs: `q6_hold.py` on r52's profiles and `q6_split.py` on r59's.
