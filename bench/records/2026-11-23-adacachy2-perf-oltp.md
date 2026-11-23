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

# Lane W3 (oltp), 2026-11-23 22:06Z to 23:06Z: the flusher with no gathering wait

## Verdict, losses first

- **update_plain at 32 clients may have lost throughput.** Median 5,476 to 4,854 writes a second
  (-11%), p50 4.80 to 5.82 ms. A's two best runs (7,585 and 8,064) beat every B run (best 5,441).
  A rank-sum over 5 runs a side gives U = 6, not significant (critical value 2), on a machine at
  load 20 to 44 on 32 cores. Rows a flush fell from 24.2 to 15.8.
- **At 1 client the p50 gain is not measurable at this noise.** p50 3.360 to 3.264 ms (median of 5,
  ranges 2.976 to 3.872 and 2.880 to 3.488). The gap to SQLite in the same round went from 0.128 to
  0.096 ms at median, not the 0.19 ms expected. The shim shows the wait gone: the last log write's
  end to the fdatasync, p50 268.8 to 21.0 µs. The fdatasync itself differed by 286 µs at p50 between
  the two traced runs (3,532 and 3,246 µs), more than the stage removed.
- **One B run stalled inside a flush.** r3-B-insert_one-32: 423 writes a second, p99 942 ms. Its
  longest flush took 930 ms (the store's `longest_us`), and its flushes took 1,444 ms of the 1.5 s
  run, so the flusher sat in fdatasync, not waiting for a wake-up. A's runs show the same kind of
  flush stall at 83 to 109 ms.
- **Merged.** 81fc0dca is on main in 6a3e8ebb (22:39Z): `Sends::gathering`, its timed wait and its
  fractions are deleted, and docs/design/store.md decision 14 holds the rule.
- At 128 clients, no measurable difference: insert_one 14,285 to 16,671 at median, update_plain
  17,788 to 15,382, ranges overlapping.

## The runs (r1657, adacachy2, A = main 98bd0fa9, B = 81fc0dca, 5 rounds alternated)

`ab-w3.sh` and `ab_w3.py` in this record's folder. W2's untraced suite command (`--suite --items
200000 --seconds 1 --warmup 0.5`), each run a fresh process on a fresh database, A's calibration
under both arms' keys. Load 19.7 to 44.2 on 32 cores, CPU pressure (avg10) 4 to 37%, I/O pressure 1
to 5%, printed beside every run. 0 errors and 0 retries in every run. Rows a flush are the store's
own counters, which cover the warm-up and the timed window (a load-only run reads 0).

| statement | clients | writes/s A | writes/s B | p50 ms A | p50 ms B | p99 ms A | p99 ms B | rows/flush A | rows/flush B |
|---|---|---|---|---|---|---|---|---|---|
| insert_one | 1 | 247 (205-302) | 261 (201-314) | 3.360 (2.976-3.872) | 3.264 (2.880-3.488) | 8.19 (5.50-17.66) | 8.00 (5.57-22.78) | 1 | 1 |
| insert_one | 32 | 5,135 (3,641-5,189) | 4,905 (423-5,289) | 5.824 (5.568-8.192) | 5.824 (5.440-14.464) | 16.64 (12.03-20.74) | 13.82 (10.88-942.08) | 20.8 (18.9-24.6) | 15.8 (14.3-16.0) |
| insert_one | 128 | 14,285 (9,595-16,241) | 16,671 (9,308-17,619) | 8.448 (7.360-12.416) | 6.720 (6.464-13.184) | 17.41 (13.95-29.18) | 17.15 (12.42-31.74) | 63.2 (60.2-65.1) | 57.3 (51.6-58.8) |
| update_plain | 32 | 5,476 (3,283-8,064) | 4,854 (4,310-5,441) | 4.800 (3.456-7.488) | 5.824 (5.504-6.976) | 14.85 (8.19-37.89) | 13.95 (10.37-24.32) | 24.2 (20.8-29.5) | 15.8 (15.6-17.7) |
| update_plain | 128 | 17,788 (10,078-19,911) | 15,382 (9,893-19,456) | 6.720 (5.824-12.032) | 7.296 (5.952-10.624) | 15.74 (11.14-21.76) | 18.94 (10.88-45.06) | 67.9 (63.1-76.1) | 63.2 (58.8-63.9) |

Median (range) of 5 runs. SQLite's insert_one at 1 client, same rounds: 258 (184-288) writes a
second, p50 3.072 (2.912-3.680) ms.

## Where the batches got smaller (traced, insert_one at 32 clients, one run a side)

- A: flush to flush p50 5,485 µs, of which fdatasync 3,569 µs. About 1.9 ms passes between one
  flush's end and the next's start: the gathering wait. 24.3 rows a flush (6,843 rows, 282 flushes).
- B: flush to flush p50 3,424 µs, fdatasync 3,386 µs. The flushes run back to back. 15.9 rows a
  flush (6,271 rows, 395 flushes).
- Writer commits a flush, from the calls counted: 520 log writes over 257 flushes in A (2.02), 688
  over 361 in B (1.91). stages.py's per-cycle count reads 0 for B, because it counts writes between
  one flush's end and the next's start, and B leaves no gap.
- Throughput is rows a flush over the mean cycle: 24.3 / 5.32 ms is 4.6k a second for A and
  15.9 / 3.79 ms is 4.2k for B. The two runs measured 4,630 and 4,164.

## Insert_one at 1 client, traced (one run a side, p50 µs)

| stage | A | B |
|---|---|---|
| writer woke to its log write (apply) | 175.3 | 82.6 |
| log write end to flusher woke | 257.6 | 14.3 |
| log write end to fdatasync | 268.8 | 21.0 |
| fdatasync | 3,532.5 | 3,246.4 |
| fdatasync end to waking the client | 6.5 | 4.6 |
| wake to the client running | 187.0 | 128.9 |
| cycle, flush end to flush end | 4,186.1 | 3,433.9 |

The apply and wake stages vary with CPU pressure (7 to 14% in these two runs). The A/B's p50 spread
at 1 client (0.9 ms in A) is several times the 0.25 ms stage B removes.

## Checks

- Pre-merge tree 661ce5b5 (r1653) and merged tree 6a3e8ebb (r1657), adacachy2:
  `cargo clippy -p amphora --all-targets --locked -- -D warnings` rc 0; `--test store` 15 passed;
  `--test crash` 9 passed, 3 ignored (not run).

## Next, for the successor

- Re-run `ab-w3.sh` on a quiet adacachy2 (load under 2, Amphora's CLAUDE.md §7) to settle 1 client,
  where 0.19 to 0.25 ms is under this run's spread, and update_plain at 32 clients.
- The flusher still waits on a 1 s timeout when idle (`flusher`, the `Duration::from_secs(1)` wait),
  as does the notifier. Neither delays a flush; both are timers under RULES 2.
