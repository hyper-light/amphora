# Lane W4 (oltp), 2026-11-23 23:07Z to 00:07Z: two flushers and a failed flush; W3's A/B on Mac Studio 2

Ada's goal: Amphora about 10x faster than every competitor, with no loss of durability. Lane W3
removed the flusher's timed gathering window (81fc0dca, on main in 6a3e8ebb) and left two things
open: a possible durability hole with more than one flusher, found by reading the code, and an A/B
too noisy at load 20 to 44 on adacachy2 to say whether update_plain lost 11% at 32 clients.

## Verdict, losses first

- **W3's flusher loses at 32 clients, and the loss is real.** On a quiet Mac Studio 2 (load 1.5 to
  2.9 on 32 cores), 5 runs a side alternated: insert_one 3,688 to 2,727 writes a second (-26%,
  rank-sum U 1, exact two-sided p 0.016), update_plain 4,353 to 3,082 (-29%, U 0, p 0.008). At 128
  clients update_plain loses 35% (14,626 to 9,462, p 0.008); insert_one shows no difference (10,843
  and 10,845, p 1.0). p50 rises with it: update_plain at 32 clients 7.17 to 10.24 ms.
- **Where the batches shrink: each flush carries half the clients.** Rows a flush fall from 29.0 to
  15.5 (insert_one, 32 clients), 29.7 to 15.3 (update_plain, 32) and 121 to 61 (update_plain, 128):
  about half the clients each time. A flush costs the same in both arms (4.6 to 4.9 ms of
  F_FULLFSYNC). The gathering flusher flushed 55 to 66% of the time and waited the rest; W3's flushes
  back to back, 81 to 96% of the time, but each flush covers only what was committed during the one
  before. Rows a flush times flushes a second accounts for the throughput: update_plain at 32
  clients, A 29.7 x 144 a second = 4,280 (measured 4,353), B 15.3 x 205 = 3,132 (measured 3,082).
  Commits a flush fall to 1.04 to 1.16 at 128 clients: one commit of the writer a flush. Read from the
  store's counters; which clients each flush covered was not traced (the stages shim is Linux-only).
  Rows a flush at half the clients, one commit a flush, fits decision 14's account of writers that
  settle into two halves, each committing during the other's flush.
- **At 1 client W3's flusher wins, and the win is real.** insert_one 152 to 188 writes a second (+24%,
  p50 6.21 to 5.18 ms), update_plain 154 to 205 (+33%, p50 6.21 to 4.48 ms), p 0.008 each. Outside its
  flush an insert spent 1.71 ms in A and 0.52 ms in B (1/writes a second less the mean flush).
- **The two-flusher durability hole was real; fixed and merged** (below).
- SQLite's insert_one on macOS (4,711 writes a second, p50 0.065 ms) is no durable comparison: the
  suite's `synchronous = FULL` doesn't flush the drive's cache on macOS without `PRAGMA fullfsync`,
  where Amphora's flush is F_FULLFSYNC. Not changed here.

## Two flushers and a failed flush (decision 9 of docs/design/store.md)

**The hole was real, and is fixed.** With two flushers (`Config::flushers`, 1 by default; the bench
sets it from `AMPHORA_FLUSHERS`), each flusher made durable what its own flush covered as soon as
that flush succeeded. A flush that failed and a flush begun before the failure was known could cover
the same commits, and the second's success acknowledged them, though the disk may have dropped them
with the first (Linux marks the pages a failed write couldn't write clean; decision 9).

The test (`crates/amphora/tests/crash.rs`,
`a_flush_that_succeeds_beside_a_failed_one_acknowledges_nothing_the_disk_dropped`, 7f4f464b) runs
the store on the crash test's simulated disk, which can now fail a flush and drop what it was making
durable. P is put and flushed; A's flush is held on one flusher, C's on the other, and D waits with
no flusher free. A's flush fails on the disk, and the store isn't told yet; C's flush succeeds and
its flusher begins D's; the store is asked whether A and C are durable; then A's failure reaches the
store, the power is cut, and the database is opened on what survived. Every batch acknowledged
durable must be there.

- Red, the test on store.rs before the fix (r1736, adacachy2, 23:24Z): `batch 1 was acknowledged
  durable and isn't on the disk after a restart (there: [0])`. A was acknowledged durable; only P
  survived.
- The fix (7b3eeeab, `Claims` in `crates/amphora/src/store.rs`): flushes are made durable in the
  order they began. A flush's target is made durable only once it and every flush begun before it
  have ended well. A failed flush never ends well, so nothing begun after it is made durable. A run
  of flushes that ended well is kept as its last, so at most two entries a flusher are kept. With
  one flusher (the default), a flush's target is made durable as it ends, as before.
- Green, the fix (r1736, adacachy2, 23:25Z): the test passes; `--test crash` 10 passed, 3 ignored
  (not run); `--test store` 15 passed; `cargo clippy -p amphora --all-targets --locked -- -D
  warnings` rc 0. The merged trees: 8f6ce397 (r1753) and 77b100cb (r1761), the same.

Not changed, for the next lane: `Db::flusher` (`crates/amphora/src/db.rs`) publishes the engine's
durable-prefix proof (`wal_prefix_durable`, decision 15) as soon as its own flush succeeds, so with
two flushers the second's success marks the log durable past what a failed first flush dropped; the
checkpointer copies up to that proof. Read in the code; no run shows it.

## W3's A/B on Mac Studio 2 (r1745, 23:32Z to 23:37Z)

`ab-w4.sh` in this record's folder (W3's `ab-w3.sh` for macOS: no pressure stall information, so
beside each run the load and the CPU all processes take; the Linux-only stages shim not run), built
by r1715. A = main 98bd0fa9, B = 81fc0dca, both built in one clone and target dir with W3's release
settings. 5 rounds alternated (A,B then B,A); each run a fresh process on a fresh database
(`--suite --only <statement>$ --items 200000 --clients <n> --seconds 1 --warmup 0.5`); no
calibration file was written by either arm's load-only run, so none was shared. Load 1.5 to 2.9 on
32 cores and CPU of all processes 27 to 446% (100 a core), each sampled just before its run. 0 errors and 0 retries in every run.
`ab_w4_stats.py` makes the table and the rank-sum tests. The store's counters cover the warm-up
and the timed window (1.5 s); the load-only runs read 0.

| statement | clients | arm | writes/s | p50 ms | p99 ms | rows/flush | commits/flush | ms/flush | flushing share |
|---|---|---|---|---|---|---|---|---|---|
| insert_one | 1 | A | 152 (151 to 155) | 6.208 (6.208 to 6.208) | 10.50 (9.73 to 11.52) | 1.0 | 1.00 | 4.87 (4.82 to 4.90) | 0.76 (0.75 to 0.76) |
| insert_one | 1 | B | 188 (187 to 219) | 5.184 (4.224 to 5.184) | 10.37 (8.58 to 10.50) | 1.0 | 1.00 | 4.80 (4.44 to 4.93) | 0.93 (0.92 to 0.98) |
| insert_one | 32 | A | 3,688 (3,083 to 3,840) | 8.064 (7.360 to 10.496) | 15.36 (12.67 to 18.69) | 29.0 (24.1 to 31.8) | 1.79 (1.51 to 1.96) | 4.66 (4.60 to 5.40) | 0.61 (0.57 to 0.68) |
| insert_one | 32 | B | 2,727 (2,699 to 3,223) | 10.624 (8.576 to 11.264) | 18.69 (17.92 to 21.25) | 15.5 (14.7 to 15.5) | 1.10 (1.07 to 1.38) | 4.93 (4.38 to 5.04) | 0.91 (0.91 to 0.98) |
| insert_one | 128 | A | 10,843 (10,399 to 12,032) | 11.648 (9.856 to 12.160) | 18.94 (16.00 to 19.46) | 100.1 (81.5 to 111.9) | 1.53 (1.32 to 1.75) | 4.87 (4.79 to 4.89) | 0.55 (0.51 to 0.64) |
| insert_one | 128 | B | 10,845 (10,192 to 11,583) | 11.264 (10.368 to 11.776) | 20.74 (18.94 to 21.50) | 59.6 (58.9 to 60.6) | 1.04 (1.02 to 1.10) | 4.65 (4.51 to 4.70) | 0.85 (0.82 to 0.89) |
| update_plain | 1 | A | 154 (152 to 162) | 6.208 (6.144 to 6.208) | 11.26 (10.37 to 11.78) | 1.0 | 1.00 | 4.84 (4.75 to 4.89) | 0.77 (0.76 to 0.78) |
| update_plain | 1 | B | 205 (194 to 218) | 4.480 (4.224 to 5.184) | 9.60 (8.45 to 11.52) | 1.0 | 1.00 | 4.67 (4.47 to 4.89) | 0.96 (0.95 to 0.98) |
| update_plain | 32 | A | 4,353 (3,700 to 4,448) | 7.168 (7.040 to 8.192) | 14.72 (11.65 to 16.38) | 29.7 (28.7 to 31.6) | 1.90 (1.80 to 1.97) | 4.58 (4.51 to 4.68) | 0.66 (0.66 to 0.70) |
| update_plain | 32 | B | 3,082 (2,793 to 3,267) | 10.240 (9.344 to 10.496) | 16.90 (16.64 to 17.92) | 15.3 (15.2 to 15.4) | 1.56 (1.53 to 1.65) | 4.69 (4.41 to 5.01) | 0.96 (0.95 to 0.96) |
| update_plain | 128 | A | 14,626 (13,970 to 16,780) | 7.552 (7.296 to 8.128) | 14.34 (12.54 to 20.22) | 121.2 (112.1 to 123.1) | 1.82 (1.74 to 1.89) | 4.55 (4.51 to 4.89) | 0.58 (0.55 to 0.66) |
| update_plain | 128 | B | 9,462 (9,094 to 13,191) | 12.928 (9.216 to 13.312) | 21.50 (19.97 to 22.02) | 60.7 (59.5 to 62.2) | 1.16 (1.14 to 1.43) | 4.76 (4.30 to 4.87) | 0.81 (0.78 to 0.92) |

Median (range) of 5 runs. SQLite's insert_one at 1 client, same rounds: 4,711 (3,899 to 5,048)
writes a second, p50 0.065 ms (not a durable comparison on macOS, above).

Rank-sum, B against A, 5 runs a side (exact two-sided p over all 252 splits):

| statement | clients | writes/s, median change | U | p | p50, median change | U | p |
|---|---|---|---|---|---|---|---|
| insert_one | 1 | +23.7% | 0 | 0.008 | -16.5% | 0 | 0.008 |
| insert_one | 32 | -26.1% | 1 | 0.016 | +31.7% | 1 | 0.016 |
| insert_one | 128 | 0.0% | 12 | 1.000 | -3.3% | 9 | 0.508 |
| update_plain | 1 | +33.1% | 0 | 0.008 | -27.8% | 0 | 0.008 |
| update_plain | 32 | -29.2% | 0 | 0.008 | +42.9% | 0 | 0.008 |
| update_plain | 128 | -35.3% | 0 | 0.008 | +71.2% | 0 | 0.008 |

The limiter in both arms is the flush: one F_FULLFSYNC of 4.6 to 4.9 ms at a time, the flusher
flushing 55 to 96% of the window (the store's counters). The CPU of each run was not measured.
Inferred, not measured here: doubling needs more rows a flush, or flushes that overlap on this disk
(decision 6: on fedora they serialized).

# Lane W5 (oltp), 2026-11-23 23:47Z to 00:47Z: a gather on events, never a clock

Ada's goal: Amphora about 10x faster than every competitor, with no loss of durability. W4 found W3's
flusher (a flush as soon as none runs) losing 26 to 35% at 32 and 128 clients, the writers split in
two halves. The timer that kept them together is forbidden. Lane W5 looked for a gather that waits
only on events.

## Verdict, losses first

- **Not merged: the A/B is incomplete.** r1815 (four arms, 5 rounds alternated, `ab-w5.sh`) was hit
  from round 2 on by other work on Mac Studio 2: a rustc build and three `mantle-a148` processes
  (`ps`, 00:22Z), load up to 47, other processes at 1,000 to 2,900% CPU. Flushes collapsed in every
  arm in those runs: T's update_plain at 1 client made 7 writes a second, M's insert_one at 1 client 3.
  Only round 1 (all arms) and round 2's D2, D1 and M runs before it are clean. The rerun into
  `out-w5b` could not start: from 00:24Z kiln refused every run ("not connected to the kiln
  server"). The checks run r1800 (`w5-checks.sh`: clippy, the store, crash and lib store tests, and
  the first new test's red run on main) was still building on MacBook Pro 2 at the end.
- **In the clean runs, D1 alone keeps the writers in one group, and beats the timer and main.**
  Rows a flush 31.0 to 32.0 at 32 clients and 123.5 to 128.0 at 128, against main's 14.0 to 15.6
  and 59.1 to 64.3. insert_one at 32 clients 5,856 and 5,792 writes a second, against main's 3,406
  and 3,190 and the timer's 4,800. At 1 client no loss against main (216 and 220 against 213 and 175).
  Two runs an arm and one of the timer: no p value, so no claim.
- **My guess about D1 was wrong.** I predicted D1 would keep the two halves, since the told senders
  haven't handed their next batches when a flush ends. In the one-group state nothing is committed
  during a flush, so the flusher's note is taken at the first commit after it, and the rest of the
  told senders hand theirs during that commit (rows a flush 32 of 32). Inferred from rows a flush and
  the code; the per-stage timestamps were not taken (the stages shim is Linux-only).
- **D2 measured no better than D1** in the clean runs, so it was reverted (0f3c6906): its waiter
  registry is code with nothing measured to show for it.

## Candidates, each a flush begun on events alone

- **D1** (716ede4d): once something is committed that no flush has begun for, the flusher notes how
  many batches have been handed to the writer and waits until that many are settled (committed,
  refused, or let go). Bounded: each is a commit of the writer away; the store stopping or its disk
  failing ends the wait; batches handed after the note aren't waited for.
- **D2** (386ab899, reverted): D1, and after a flush the flusher waits for the senders it told to
  leave their wait in `Store::durable`, counted by commit. Bounded: each leaves on its own thread.
- **Refused on paper.** Waiting for the told senders to send again (a sender that stops never does,
  and nothing else ends the wait). Waiting for the writer's queue to empty (senders that don't wait,
  a bulk load among them, can keep it full).
- **The flusher's wait for a commit** looked again every second; every batch settled, the store
  stopping and a failed flush notify it under its lock, so 0e83fc98 makes it wait on those alone.
  Not in the measured binaries, and not yet tested (r1800 tested the tree before it).

## The clean runs (r1815, 00:12Z to 00:16Z, load 3.4 to 9.2 on 32 cores)

T = 98bd0fa9 (the timer), M = f363d688 (main, the candidates' parent), D1 = 716ede4d, D2 = 386ab899.
Each binary's sha256 printed first (all four differ). Writes a second (p50 ms; rows a flush), each run
a fresh process on a fresh database, 0 errors and 0 retries.

| statement | clients | T r1 | M r1, r2 | D1 r1, r2 | D2 r1, r2 |
|---|---|---|---|---|---|
| insert_one | 1 | 169 (5.25; 1.0) | 213 (4.22), 175 (5.18) | 216 (4.22), 220 (4.22) | 220 (4.22), 218 (4.22) |
| insert_one | 32 | 4,800 (6.21; 31.9) | 3,406 (8.45; 14.8), 3,190 (8.58; 14.0) | 5,856 (5.18; 32.0), 5,792 (5.18; 31.7) | 5,824 (5.18; 32.0), 5,696 (5.25; 32.0) |
| insert_one | 128 | 13,184 (9.34; 123.0) | 12,041 (9.60; 59.1), 12,027 (9.73; 60.1) | 15,488 (8.19; 127.3), 14,592 (8.32; 128.0) | 15,104 (8.32; 128.0), 15,360 (8.32; 128.0) |
| update_plain | 1 | 159 (5.82; 1.0) | 206 (4.22), 226 (4.22) | 223 (4.22), 229 (4.22) | 224 (4.22), 223 (4.22) |
| update_plain | 32 | 4,896 (6.21; 31.6) | 3,584 (8.45; 15.6), 3,566 (8.45; 15.6) | 6,641 (4.22; 31.0), 6,455 (4.22; 31.4) | 6,400 (5.12; 32.0), 6,496 (5.06; 32.0) |
| update_plain | 128 | 18,688 (6.46; 127.4) | 12,923 (9.34; 63.5), 8,636 (14.46; 64.3) | 20,880 (6.14; 126.6), 16,238 (6.59; 123.5) | 20,736 (6.14; 127.8), 20,992 (6.14; 127.7) |

Round 2's M and D1 update_plain at 128 may already carry the interference (p99 23.8 and 15.5 ms).
The disturbed runs' JSON stays in `~/.cache/oltp-ab/out-w5` on Mac Studio 2.

## For the next lane

1. Rerun `ab-w5.sh` (arms built, `OUT=$HOME/.cache/oltp-ab/out-w5b`) on a quiet Mac Studio 2 and
   read `ab_w5_stats.py`'s p values for M against D1 and T against D1 at 1, 32 and 128 clients.
2. Run `w5-checks.sh` (`RED=f363d688`) on perf/oltp's tip.
3. If D1 holds, fill decision 14's evidence (docs/design/store.md) and merge perf/oltp.

## The rerun (r1888, 00:28Z to 00:39Z) and both runs kept by the disk's quiet flush cost

r1888 (`OUT=out-w5b`, the same arms and binaries) was disturbed in rounds 1, 3, 4 and 5 by the same
outside work (load to 47, others to 2,900% CPU); unfiltered, its mean flush reached 143 ms against
the disk's 4.6. Rows a flush, which slow flushes can't push past all clients, held in all 5 runs a
side: D1 32.0 and 31.4 at 32 clients, 127.2 and 126.1 at 128; main 14.7, 15.0, 60.0 and 60.4.

`ab_w5_quiet.py` keeps, from both runs, the runs whose mean flush is at most W4's largest quiet mean
on this machine (5.40 ms): a flush is one F_FULLFSYNC whose cost W4 measured the same in both arms.
The filter was written after the runs, and some cells keep fewer than 5 runs a side, so this is
evidence for the next lane, not a merge gate. Median (range), writes a second:

| statement | clients | T | M | D1 | D1 against M |
|---|---|---|---|---|---|
| insert_one | 1 | 169 (158 to 196), 5 runs | 205 (175 to 218), 5 | 214 (205 to 220), 5 | +4.4%, p 0.302 |
| insert_one | 32 | 4,712 (3,872 to 4,800), 5 | 3,138 (2,945 to 3,406), 6 | 5,456 (4,896 to 5,856), 6 | +73.9%, p 0.002 |
| insert_one | 128 | 12,066 (11,361 to 13,184), 4 | 12,027 (10,004 to 12,242), 5 | 14,272 (13,736 to 15,488), 6 | +18.7%, p 0.004 |
| update_plain | 1 | 159 (157 to 199), 4 | 210 (198 to 226), 5 | 216 (213 to 229), 5 | +2.9%, p 0.278 |
| update_plain | 32 | 4,858 (3,825 to 5,312), 4 | 3,566 (3,246 to 3,603), 5 | 5,829 (5,667 to 6,641), 7 | +63.5%, p 0.003 |
| update_plain | 128 | 17,240 (12,160 to 18,688), 4 | 12,923 (11,725 to 13,882), 3 | 18,433 (15,958 to 20,880), 7 | +42.6%, p 0.017 |

Rows a flush in the kept runs: D1 32.0, 127.2, 31.4, 123.5; M 14.6, 60.1, 15.6, 63.5; T 31.9, 98.6,
30.9, 123.0 (insert_one 32, 128, update_plain 32, 128). p50 at 32 clients: D1 5.28 and 5.12 ms, M
8.83 and 8.45, T 6.34 and 6.18. Mean flush 4.28 to 5.07 ms in every arm. 0 errors, 0 retries.

# Lane W6 (oltp), 2026-11-24 00:42Z to 01:42Z: D1 landed, and its A/B on a quiet Mac Studio 2

Ada's goal: Amphora about 10x faster than every competitor, with no loss of durability. Lane W6
lands lane W5's gather (D1, decision 14 of docs/design/store.md) on main, and measures it against
main before it and the old timer, 5 rounds, every run kept.

## Verdict, losses first

- **The A/B measured W5's D1 binary (716ede4d), not main's merged code.** The merged tree adds
  0e83fc98 (the flusher's 1-second re-look removed) and main's memory-budget charge (A16's
  5e5ae655, A18's bdfb19ff) in the same file. Neither is in the timed binaries.
- **D1 against the timer, update_plain at 32 and 128 clients: no measurable difference.** Medians
  +20.4% and +8.8%, p 0.151 each. Two D1 runs sit low: r2 at 32 clients (4,384 writes a second,
  beside other processes' CPU at 873 to 945%) and r1 at 128 (15,090, cause not found).
- **The design index fails 3 of 9 on this tree, every fault main's own** (below).
- **D1 against main: faster at 32 and 128 clients, the same at 1.** Writes a second +77.3% and
  +75.3% at 32 clients, +25.4% and +50.5% at 128 (p 0.008 each); +0.5% and +0.9% at 1 (p 0.81 and
  0.86). p50 at 32 clients 5.18 and 5.06 ms against 8.45.
- **D1 against the timer: faster at 1 client and for insert_one at 32 and 128.** +29.1% and +27.3%
  at 1 client, +23.3% and +12.5% for insert_one at 32 and 128, p 0.008 each.
- **Rows a flush: every client in every flush.** D1 32.0 and 31.6 at 32 clients, 128.0 and 125.6
  at 128; main 14.5, 15.6, 60.1 and 62.7; the timer 31.2, 31.6, 116.9 and 123.8.

## The checks

r1902 (MacBook Pro 2, W5's `w5-checks.sh` on W5's merge ac068dd1): clippy finished with no error;
`--test store` 17 passed, both new tests among them; `--test crash` 10 passed, 3 ignored by design;
`--lib store` 8 and 1 passed. Red run on main's store.rs (f363d688), 3 of 3:
`a_batch_handed_before_a_flush_begins_is_made_durable_by_it` fails with "x and y were made durable
by one flush after the first, left 3, right 2": main takes a third flush where D1 takes two. Its
filter hid the script's own `==` lines, so pass and fail are read from cargo's lines.

Main moved in crates/amphora (store.rs: A16's and A18's memory budget), crates/engine and records,
so the merge a47b071d (main 7d6cd7f0) got its own run. Read on paper first: a batch refused for
room is refused before `Sends::handing`, so it never counts in the flusher's note; a failed send
settles and wakes; the writer wakes for every batch it drains. r1957 (`w6-checks.sh`, the same
checks reordered so each build is reused, `RED` = main 7d6cd7f0): `--test store` 16 passed, both
new tests among them; `--test crash` 10 passed, 3 ignored; `--lib store` 10 and 1 passed, A18's two
wake tests among them; clippy finished with no error (the script's rc 0 over these four, 01:20Z).
Red runs on main's store.rs (7d6cd7f0), 3 of 3: the first new test fails, left 3, right 2; the
second passes, as it does on any design that never waits for a told sender to send again (it fails
only on one that does, which decision 14 refuses on paper).

Main then moved in crates/engine (Q3's log segments), so the merge 1c3b7c84 (main 57fd89cd) got
r1977: `--test store` 16 passed, `--test crash` 10 passed and 3 ignored, clippy rc 0 (01:37Z).
Main moved again only in a record; merged and landed on main as 8e6bcca6 (01:40Z).

The design index fails the same 3 tests on W5's tree (r1902) and on this one (r1957). Each fault
names a decision with no index line, or a quote, from other lanes: local-owner-transport version
10, planner T13, replication P6, store perf-A9, perf-A18 and 45, and the client protocol's "Answers
past one frame". None is decision 14, which has its line (docs/design.md:221). On origin/main no
index line names any of them.

## The A/B (r1959, Mac Studio 2, 01:07Z to 01:13Z)

`ab-w6.sh`: W5's `ab-w5.sh` with three arms, T the timer (98bd0fa9), M main before D1 (f363d688),
D1 (716ede4d), binaries as W5 built them; insert_one and update_plain at 1, 32 and 128 clients, 5
rounds, the arms' order reversed every other round, each run a fresh process on a fresh database;
the first arm's calibration under every arm's key (RULES 5e). `ab_w6_runs.py` puts each run's mean
flush and the load beside it; `ab_w5_stats.py` now also tests D1 against T.

The machine. At 00:56Z Mac Studio 2 was not quiet: another lane's four `mantle-a148` at 50 to 231%
CPU each, load 9.0, the disk at 25,600 transfers a second (105 MB/s of 4 KB writes). The other
Studios have 14 cores and 0.3 and 6 GB free, short of the run's cpus=16; 2768d0d5 is excluded.
W5's flush bound (5.40 ms) was named before the run as the fallback. At 01:07Z, when r1959 began,
the `mantle-a148` were gone: load 2.1 on 32 cores, the disk at 160 to 355 transfers a second (2.5 to
3.8 MB/s), one other process (a `slates` release binary under `~/.cache/main-rounds`) at about
one core throughout. Through the runs: load 2.28 to 5.24, CPU of all processes 13 to 945% (100 a core),
every run's mean flush 4.14 to 5.34 ms. No run passed the bound, so the bounded table equals the
one below. 0 errors, 0 retries in all 90 runs.

Writes a second, median (range) of 5 runs; D1's change against M and T with the exact two-sided
rank-sum p (5 against 5); rows a flush and mean flush ms, T, M, D1:

| statement | clients | T | M | D1 | D1 against M | D1 against T | rows/flush | flush ms |
|---|---|---|---|---|---|---|---|---|
| insert_one | 1 | 172 (171 to 177) | 221 (215 to 226) | 222 (185 to 226) | +0.5%, p 0.810 | +29.1%, p 0.008 | 1.0, 1.0, 1.0 | 4.51, 4.47, 4.42 |
| insert_one | 32 | 4,751 (4,576 to 4,961) | 3,303 (3,233 to 3,447) | 5,856 (5,663 to 5,920) | +77.3%, p 0.008 | +23.3%, p 0.008 | 31.2, 14.5, 32.0 | 4.40, 4.35, 4.28 |
| insert_one | 128 | 13,315 (11,891 to 13,445) | 11,938 (11,288 to 12,592) | 14,976 (14,592 to 15,488) | +25.4%, p 0.008 | +12.5%, p 0.008 | 116.9, 60.1, 128.0 | 4.90, 4.41, 4.90 |
| update_plain | 1 | 176 (165 to 184) | 222 (209 to 227) | 224 (204 to 227) | +0.9%, p 0.857 | +27.3%, p 0.008 | 1.0, 1.0, 1.0 | 4.44, 4.43, 4.42 |
| update_plain | 32 | 5,250 (5,120 to 5,504) | 3,606 (3,433 to 3,655) | 6,321 (4,384 to 6,747) | +75.3%, p 0.008 | +20.4%, p 0.151 | 31.6, 15.6, 31.6 | 4.29, 4.31, 4.24 |
| update_plain | 128 | 18,419 (17,571 to 18,975) | 13,306 (13,044 to 13,604) | 20,032 (15,090 to 21,319) | +50.5%, p 0.008 | +8.8%, p 0.151 | 123.8, 62.7, 125.6 | 4.37, 4.30, 4.33 |

p50 ms, median of 5:

| statement | clients | T | M | D1 | D1 against M | D1 against T |
|---|---|---|---|---|---|---|
| insert_one | 1 | 5.248 | 4.224 | 4.224 | +0.0%, p 1.000 | -19.5%, p 0.016 |
| insert_one | 32 | 6.208 | 8.448 | 5.184 | -38.6%, p 0.008 | -16.5%, p 0.008 |
| insert_one | 128 | 9.344 | 9.600 | 8.320 | -13.3%, p 0.008 | -11.0%, p 0.008 |
| update_plain | 1 | 5.248 | 4.160 | 4.224 | +1.5%, p 0.119 | -19.5%, p 0.008 |
| update_plain | 32 | 6.208 | 8.448 | 5.056 | -40.2%, p 0.008 | -18.6%, p 0.151 |
| update_plain | 128 | 6.464 | 9.344 | 6.208 | -33.6%, p 0.016 | -4.0%, p 0.111 |

Main against the timer, the same runs: +28.5% and +26.1% at 1 client, -30.5% and -31.3% at 32,
-10.3% (p 0.056) and -27.8% at 128: W4's finding again.

**The limiter is the flush.** In every cell, writes a second match rows a flush times flushes a
second (the flusher's flushing share of the 1.5 s window over the mean flush) within 3% (0.0 to
2.8% in the 18 cells): D1 at 32
clients 32.0 x 0.79 / 4.28 ms = 5,907 against 5,856 measured; main 14.5 x 0.98 / 4.35 ms = 3,266
against 3,303. At 1 client a write waits for its own flush: D1 and main at 222 to 224 a second,
flushing 98% of the window, at most 2% under one 4.4 ms flush at a time; the timer flushes 78% of
it (its wait, p50 +1.02 ms). At 32 clients D1 already flushes all 32 writers each time; the
flusher's other 21% is the wait for the 2.0 commits a flush of the batches it counted (inferred
from the store's counters; no per-stage trace on macOS). The CPU of all processes stayed near one
core of 32 in most runs, so the work waits on the disk. Doubling needs more writers a flush or
flushes that overlap on this disk (decision 6).

Every run, in order:

| run | arm | statement | clients | writes/s | mean flush ms | rows/flush | load before, after | CPU % before, after |
|---|---|---|---|---|---|---|---|---|
| r1 | T | insert_one | 1 | 171.0 | 4.60 | 1.0 | 2.40, 2.44 | 120, 125 |
| r1 | T | insert_one | 32 | 4751.0 | 4.40 | 31.2 | 2.44, 2.73 | 125, 135 |
| r1 | T | insert_one | 128 | 13445.0 | 4.69 | 105.9 | 2.73, 2.59 | 136, 125 |
| r1 | T | update_plain | 1 | 165.0 | 4.68 | 1.0 | 2.59, 2.59 | 123, 123 |
| r1 | T | update_plain | 32 | 5120.0 | 4.55 | 31.6 | 2.59, 2.54 | 135, 121 |
| r1 | T | update_plain | 128 | 17571.0 | 4.37 | 123.8 | 2.54, 2.42 | 122, 109 |
| r1 | M | insert_one | 1 | 218.0 | 4.53 | 1.0 | 2.42, 2.39 | 110, 135 |
| r1 | M | insert_one | 32 | 3447.0 | 4.41 | 15.3 | 2.39, 2.67 | 137, 127 |
| r1 | M | insert_one | 128 | 11288.0 | 4.61 | 60.5 | 2.67, 2.67 | 127, 65 |
| r1 | M | update_plain | 1 | 209.0 | 4.60 | 1.0 | 2.67, 2.94 | 367, 120 |
| r1 | M | update_plain | 32 | 3641.0 | 4.31 | 15.6 | 2.94, 2.86 | 121, 113 |
| r1 | M | update_plain | 128 | 13467.0 | 4.20 | 64.1 | 2.86, 2.72 | 116, 122 |
| r1 | D1 | insert_one | 1 | 222.0 | 4.43 | 1.0 | 2.72, 2.72 | 123, 147 |
| r1 | D1 | insert_one | 32 | 5920.0 | 4.29 | 32.0 | 2.72, 2.74 | 138, 121 |
| r1 | D1 | insert_one | 128 | 15104.0 | 4.96 | 128.0 | 2.74, 2.76 | 126, 13 |
| r1 | D1 | update_plain | 1 | 204.0 | 4.70 | 1.0 | 2.76, 2.94 | 13, 40 |
| r1 | D1 | update_plain | 32 | 6111.0 | 4.25 | 29.9 | 2.94, 2.94 | 40, 122 |
| r1 | D1 | update_plain | 128 | 15090.0 | 4.56 | 125.6 | 2.94, 3.02 | 122, 99 |
| r2 | D1 | insert_one | 1 | 185.0 | 4.92 | 1.0 | 3.02, 2.78 | 123, 112 |
| r2 | D1 | insert_one | 32 | 5856.0 | 4.28 | 32.0 | 2.78, 2.88 | 146, 57 |
| r2 | D1 | insert_one | 128 | 14870.0 | 4.90 | 126.0 | 2.88, 2.88 | 98, 367 |
| r2 | D1 | update_plain | 1 | 224.0 | 4.42 | 1.0 | 2.88, 3.13 | 362, 873 |
| r2 | D1 | update_plain | 32 | 4384.0 | 4.83 | 31.7 | 3.13, 3.92 | 945, 106 |
| r2 | D1 | update_plain | 128 | 19981.0 | 4.50 | 126.1 | 3.92, 3.68 | 119, 121 |
| r2 | M | insert_one | 1 | 221.0 | 4.45 | 1.0 | 3.68, 3.47 | 121, 129 |
| r2 | M | insert_one | 32 | 3258.0 | 4.42 | 14.9 | 3.47, 3.47 | 132, 134 |
| r2 | M | insert_one | 128 | 12592.0 | 4.38 | 60.1 | 3.47, 3.35 | 134, 134 |
| r2 | M | update_plain | 1 | 227.0 | 4.39 | 1.0 | 3.35, 3.56 | 134, 123 |
| r2 | M | update_plain | 32 | 3655.0 | 4.20 | 15.6 | 3.56, 3.36 | 123, 132 |
| r2 | M | update_plain | 128 | 13604.0 | 4.28 | 63.5 | 3.36, 3.36 | 131, 117 |
| r2 | T | insert_one | 1 | 171.0 | 4.53 | 1.0 | 3.36, 3.33 | 122, 130 |
| r2 | T | insert_one | 32 | 4675.0 | 4.47 | 31.3 | 3.33, 3.22 | 127, 134 |
| r2 | T | insert_one | 128 | 13315.0 | 5.06 | 117.5 | 3.22, 3.04 | 132, 119 |
| r2 | T | update_plain | 1 | 176.0 | 4.54 | 1.0 | 3.04, 3.04 | 124, 132 |
| r2 | T | update_plain | 32 | 5216.0 | 4.40 | 31.8 | 3.04, 3.04 | 136, 114 |
| r2 | T | update_plain | 128 | 18014.0 | 4.47 | 123.8 | 3.04, 2.88 | 128, 111 |
| r3 | T | insert_one | 1 | 177.0 | 4.45 | 1.0 | 2.88, 2.81 | 120, 115 |
| r3 | T | insert_one | 32 | 4576.0 | 4.54 | 31.7 | 2.81, 2.81 | 116, 99 |
| r3 | T | insert_one | 128 | 12928.0 | 4.90 | 116.9 | 2.81, 2.74 | 99, 111 |
| r3 | T | update_plain | 1 | 184.0 | 4.38 | 1.0 | 2.74, 2.84 | 102, 84 |
| r3 | T | update_plain | 32 | 5250.0 | 4.29 | 30.9 | 2.84, 2.77 | 81, 91 |
| r3 | T | update_plain | 128 | 18869.0 | 4.26 | 123.8 | 2.77, 2.77 | 91, 72 |
| r3 | M | insert_one | 1 | 222.0 | 4.47 | 1.0 | 2.77, 2.71 | 76, 90 |
| r3 | M | insert_one | 32 | 3303.0 | 4.29 | 14.4 | 2.71, 2.90 | 87, 74 |
| r3 | M | insert_one | 128 | 11513.0 | 4.57 | 58.9 | 2.90, 2.74 | 68, 93 |
| r3 | M | update_plain | 1 | 220.0 | 4.49 | 1.0 | 2.74, 2.74 | 95, 86 |
| r3 | M | update_plain | 32 | 3511.0 | 4.35 | 15.6 | 2.74, 2.68 | 84, 74 |
| r3 | M | update_plain | 128 | 13044.0 | 4.39 | 61.3 | 2.68, 2.55 | 75, 75 |
| r3 | D1 | insert_one | 1 | 225.0 | 4.34 | 1.0 | 2.55, 2.50 | 75, 72 |
| r3 | D1 | insert_one | 32 | 5663.0 | 4.28 | 31.9 | 2.50, 2.50 | 79, 89 |
| r3 | D1 | insert_one | 128 | 14976.0 | 4.89 | 128.0 | 2.50, 2.54 | 97, 110 |
| r3 | D1 | update_plain | 1 | 219.0 | 4.43 | 1.0 | 2.54, 2.42 | 110, 124 |
| r3 | D1 | update_plain | 32 | 6747.0 | 4.14 | 31.6 | 2.42, 2.31 | 122, 118 |
| r3 | D1 | update_plain | 128 | 20032.0 | 4.29 | 121.6 | 2.31, 2.31 | 121, 109 |
| r4 | D1 | insert_one | 1 | 226.0 | 4.33 | 1.0 | 2.31, 2.28 | 114, 130 |
| r4 | D1 | insert_one | 32 | 5824.0 | 4.33 | 32.0 | 2.28, 2.58 | 133, 135 |
| r4 | D1 | insert_one | 128 | 15488.0 | 4.86 | 128.0 | 2.58, 2.45 | 129, 114 |
| r4 | D1 | update_plain | 1 | 227.0 | 4.35 | 1.0 | 2.45, 2.45 | 112, 123 |
| r4 | D1 | update_plain | 32 | 6321.0 | 4.24 | 31.4 | 2.45, 2.50 | 127, 123 |
| r4 | D1 | update_plain | 128 | 20400.0 | 4.27 | 125.1 | 2.50, 2.54 | 131, 114 |
| r4 | M | insert_one | 1 | 226.0 | 4.35 | 1.0 | 2.54, 2.57 | 123, 152 |
| r4 | M | insert_one | 32 | 3233.0 | 4.27 | 13.9 | 2.57, 2.57 | 152, 127 |
| r4 | M | insert_one | 128 | 11938.0 | 4.41 | 59.6 | 2.57, 2.53 | 125, 112 |
| r4 | M | update_plain | 1 | 222.0 | 4.43 | 1.0 | 2.53, 2.48 | 112, 121 |
| r4 | M | update_plain | 32 | 3606.0 | 4.31 | 15.8 | 2.48, 2.69 | 119, 61 |
| r4 | M | update_plain | 128 | 13305.0 | 4.38 | 62.7 | 2.69, 2.69 | 86, 109 |
| r4 | T | insert_one | 1 | 172.0 | 4.50 | 1.0 | 2.69, 2.79 | 122, 140 |
| r4 | T | insert_one | 32 | 4782.0 | 4.33 | 30.3 | 2.79, 2.73 | 151, 134 |
| r4 | T | insert_one | 128 | 11891.0 | 4.71 | 77.0 | 2.73, 4.83 | 137, 88 |
| r4 | T | update_plain | 1 | 175.0 | 4.44 | 1.0 | 4.83, 4.83 | 118, 517 |
| r4 | T | update_plain | 32 | 5504.0 | 4.18 | 31.7 | 4.83, 5.24 | 495, 119 |
| r4 | T | update_plain | 128 | 18419.0 | 4.37 | 109.1 | 5.24, 5.06 | 119, 118 |
| r5 | T | insert_one | 1 | 172.0 | 4.51 | 1.0 | 5.06, 4.90 | 118, 125 |
| r5 | T | insert_one | 32 | 4961.0 | 4.21 | 31.2 | 4.90, 4.90 | 122, 117 |
| r5 | T | insert_one | 128 | 13440.0 | 4.91 | 124.8 | 4.90, 4.67 | 118, 125 |
| r5 | T | update_plain | 1 | 176.0 | 4.43 | 1.0 | 4.67, 4.53 | 249, 148 |
| r5 | T | update_plain | 32 | 5411.0 | 4.17 | 31.1 | 4.53, 4.25 | 140, 136 |
| r5 | T | update_plain | 128 | 18975.0 | 4.24 | 120.4 | 4.25, 4.25 | 136, 96 |
| r5 | M | insert_one | 1 | 215.0 | 4.55 | 1.0 | 4.25, 4.07 | 97, 114 |
| r5 | M | insert_one | 32 | 3386.0 | 4.35 | 14.5 | 4.07, 3.90 | 114, 107 |
| r5 | M | insert_one | 128 | 12132.0 | 4.35 | 60.3 | 3.90, 3.67 | 111, 114 |
| r5 | M | update_plain | 1 | 222.0 | 4.43 | 1.0 | 3.67, 3.67 | 118, 119 |
| r5 | M | update_plain | 32 | 3433.0 | 4.20 | 14.9 | 3.78, 3.78 | 131, 113 |
| r5 | M | update_plain | 128 | 13306.0 | 4.30 | 60.9 | 3.78, 3.63 | 116, 113 |
| r5 | D1 | insert_one | 1 | 219.0 | 4.42 | 1.0 | 3.63, 3.42 | 114, 124 |
| r5 | D1 | insert_one | 32 | 5888.0 | 4.26 | 31.9 | 3.42, 3.23 | 125, 114 |
| r5 | D1 | insert_one | 128 | 14592.0 | 5.34 | 126.2 | 3.23, 3.23 | 118, 141 |
| r5 | D1 | update_plain | 1 | 225.0 | 4.38 | 1.0 | 3.23, 3.13 | 132, 126 |
| r5 | D1 | update_plain | 32 | 6663.0 | 4.18 | 31.8 | 3.13, 3.04 | 127, 128 |
| r5 | D1 | update_plain | 128 | 21319.0 | 4.33 | 126.6 | 3.04, 2.96 | 127, 109 |
