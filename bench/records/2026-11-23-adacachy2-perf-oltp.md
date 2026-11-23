# Lane W1 (oltp), 2026-11-23 20:15Z to 21:15Z: insert_one at 1 client

Ada's goal: Amphora about 10x faster than every competitor on every query type. S6 measured
insert_one at 1 client 7.6x slower than SQLite (45 against 343 ops/s, p50 21.5 ms against 2.59 ms,
one run, load 31-75). This lane checked that the two sides ran at the same durability, measured where
one insert's time goes, and fixed what the measurement showed.

## Verdict, losses first

- **The A/B of the fix has not run yet.** Run r1558 failed before building (a corrupted inline
  patch). The script is committed as `2026-11-23-adacachy2-perf-oltp/ab.sh` and runs from the branch.
  Its numbers belong in the table below.
- **S6's 7.6x does not reproduce on main 5f511d8d with insert_one run alone.** One run each, load
  31-34 on 32 cores (adacachy2, kiln run r1536, cpus=8): Amphora 223 ops/s, p50 3.49 ms, p99 17.2 ms;
  SQLite 285 ops/s, p50 2.94 ms, p99 14.8 ms. Amphora is 1.28x slower. One run a side: directional.
- **What made S6's insert_one take 21.5 ms is not found.** S6 ran it after 15 other statements in one
  process, at 1, 32 and 128 clients. Its run's store counters show 2,547 checkpoint copies (17.5 s)
  beside 2,611 flushes (27.1 s): about one copy every 25 ms for the whole run.

## Durability: the two sides ran at the same setting

- SQLite: `PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL` on every connection
  (`bench/src/suite_db.rs`, `Sqlite::connect`). SQLite's documentation (sqlite.org/pragma.html,
  synchronous): in WAL mode, FULL syncs the WAL after each transaction commit.
- Amphora: one `fdatasync` of the log a commit before the commit is acknowledged
  (`crates/amphora/src/db.rs`, `Db::flusher`). Its meter counted 343 flushes for 343 commits.
- Not measured: SQLite's syncs a commit. adacachy2 has no `strace`, and `perf_event_paranoid` is 2.

## Where one insert's time goes (main 5f511d8d, the store's own meter, the real run)

insert_one alone, 1 client, 200,000 items, 1 s window after 0.5 s warm-up (`amphora-bench --suite
--only 'insert_one$' --items 200000 --clients 1 --seconds 1 --warmup 0.5`):

| stage | count | total ms | ms each |
|---|---|---|---|
| commits | 343 | | |
| apply (the writer's transaction) | 343 | 77 | 0.22 |
| flush (`fdatasync` of the log) | 343 | 1,300 | 3.79 |
| checkpoint copies (own thread) | 45 | 378 | 8.4 |

The process used 0.04 s of user and 0.06 s of system CPU in the 1 s window: it waits on the disk, so
this is a per-stage timing, not a CPU profile. The flush is about 95% of an insert.

The 45 checkpoint copies are the harness's, not Amphora's. The suite's `SqlEngine` overrides
`Engine::checkpoint` but not `Engine::uncopied`, so the store can't tell how much of its log is
uncopied and copies it into the file every period (25 ms), with a flush of the file each time.
Amphora's own engine (`crates/amphora/src/database.rs`, `impl Engine for Local`) reports
`Db::uncopied`, and its checkpointer leaves a log within its bound uncopied (docs/design/store.md,
decision 35). Fix: perf/oltp e57a028b gives the suite's engine the same `uncopied`.

## The A/B (pending)

Run from a clone at perf/oltp on adacachy2:
`git show origin/perf/oltp:bench/records/2026-11-23-adacachy2-perf-oltp/ab.sh > ab.sh && bash ab.sh`.
A = main 5f511d8d, B = e57a028b. 5 rounds alternated (A,B then B,A), each run a fresh process on a
fresh database, SQLite once a round, A's calibration written under both arms' keys (RULES 5e).
kiln refused cpus=16 (13 of 32 cores idle), so the run holds cpus=8. Then clippy on the bench crate.

| arm | runs | ops/s median (range) | p50 ms median (range) | p99 ms median | checkpoint copies a run | errors |
|---|---|---|---|---|---|---|
| A main 5f511d8d | 5 | 254 (191-304) | 3.36 (2.91-4.93) | 7.49 | 42-48 | 0 |
| B e57a028b | 5 | 249 (203-289) | 3.17 (3.10-4.86) | 7.74 | 0 | 0 |
| SQLite | 5 | 318 (195-330) | 2.72 (2.59-4.80) | 10.24 | | 0 |

Filled by lane W2 from run r1561 (output in `~/.cache/amphora-perf/oltp-run/ab-e57a028b/` on
adacachy2, ended 21:14Z, load 28-37 on 32 cores). Verdict: B removes every checkpoint copy, but its
p50 and ops/s differ from A's by less than the run-to-run spread: no measurable difference at
1 client. Round 1 was slow on all three arms alike (p50 4.8-4.9 ms), so the disk, not an arm, set
it. The flush ms a flush (2.9-4.6 ms) is the meter's mean over the whole process, load included,
so it is not one insert's flush. Clippy on the bench crate exited non-zero: 162 errors, all in
files and lines e57a028b doesn't touch (`println!`, `std::env::args`, `thread::sleep` and others
in 28 files; none in `suite_db.rs` lines 220-239). The bench crate is outside the workspace and
fails `-D warnings` on main the same way (lane V's note: 164 errors).

# Lane W2 (oltp), 2026-11-23 21:17Z to 22:10Z: where Amphora's extra time per insert goes

## Verdict, losses first

- **The deliverable is not met.** insert_one at 1 client is still above SQLite's p50, and no
  speed fix landed this hour. An API outage stopped the lane from about 21:30Z to 21:50Z.
- **The cause is traced, not fixed.** After the writer's log write, the flusher waits about 192 µs
  in a timed wait that expires before it opens and flushes the log. That is the store's gathering
  window (`crates/amphora/src/store.rs` `flusher`, lines 896-929, sized by `Sends::gathering`,
  lines 794-809). At 1 client nothing can join that flush, because the only sender is waiting for
  it. The wait is two-thirds of the p50 gap.
- **W1's open-per-flush lead is ruled out.** The flusher's open of the log takes 3.2 µs and its
  close 0.9 µs a flush.

## The runs (r1595, perf/oltp 7ec436da, binary B = e57a028b, adacachy2, load 14-16 on 32 cores)

insert_one alone, 1 client, 200,000 items, 1 s after 0.5 s warm-up, 3 rounds alternated: Amphora
plain, Amphora traced, SQLite plain, SQLite traced. 0 errors in every run. `trace.sh` in this
record's folder runs it all; `stages.c` is the `LD_PRELOAD` shim; `stages.py` lays its records out
per insert, anchored on each `fdatasync` of the log. The shim costs nothing measurable: traced p50s
sit inside the plain runs' range.

| run | Amphora plain p50 ms (ops/s) | Amphora traced | SQLite plain | SQLite traced |
|---|---|---|---|---|
| r1 | 2.944 (309) | 2.912 (315) | 2.656 (326) | 2.624 (330) |
| r2 | 2.944 (306) | 2.944 (301) | 2.624 (330) | 2.624 (327) |
| r3 | 3.424 (260) | 3.264 (265) | 2.720 (232) | 2.624 (324) |

The p50 gap in plain runs is 0.29, 0.32 and 0.70 ms.

## One insert's stages, Amphora (p50 µs over 369-432 inserts a run)

| stage | r1 | r2 | r3 |
|---|---|---|---|
| writer woke to its log write (apply) | 61.8 | 63.9 | 65.1 |
| log write (one `pwritev`, 16.5 KB, 4 frames) | 11.0 | 10.5 | 11.1 |
| log write end to flusher woke (wake ~50, then the gathering wait ~192) | 197.6 | 199.8 | 211.3 |
| open of the log | 3.2 | 3.1 | 3.2 |
| `fdatasync` | 2620.8 | 2650.3 | 2944.2 |
| close | 0.9 | 0.9 | 0.9 |
| `fdatasync` end to waking the client | 3.8 | 3.8 | 3.9 |
| wake to the client running | 96.9 | 99.1 | 100.1 |
| cycle, flush end to flush end | 2913.3 | 2952.8 | 3250.9 |

SQLite, the same runs: `fdatasync` of its log p50 2581.2, 2575.3 and 2562.4 µs; cycle p50 2675.9,
2681.0 and 2663.7 µs. Cycle less flush at p50 is about 95 µs for SQLite and about 292 µs for
Amphora. SQLite's write stage is not separated: `stages.py` took its load thread for its writer.

Three sampled inserts (r1, mid-run) show the wait itself. The writer's futex wake comes 8-49 µs
after its `pwritev`. The flusher returns from its wait, then enters a `FUTEX_WAIT_BITSET` that lasts
192.8, 191.8 and 192.4 µs and returns -1 (timed out), then opens the log 3 µs later and flushes.

## Next, for the successor

- Decide the gathering wait by data (RULES 4): A/B the flusher flushing at once when no other
  sender can join, against main, at 1, 32 and 128 clients. The window exists to batch commits under
  load (the comment at store.rs:891-895), so the 32- and 128-client runs are the other side of it.
- Not traced: what sets the 192 µs. `gathering` bounds the wait by `flush / 32` when the writer is
  idle and `flush / 2` otherwise. Neither matches a 2.6-2.9 ms flush, unless the smoothed flush
  time is near 6.1 ms. Log `flush_time` and `idle_since` at each wait before changing it.
- Amphora's `fdatasync` ran 40-75 µs longer than SQLite's at p50 in r1 and r2 (16.5 KB of
  frames a commit). That is the next stage once the wait is settled.
