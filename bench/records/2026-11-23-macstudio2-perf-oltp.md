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
