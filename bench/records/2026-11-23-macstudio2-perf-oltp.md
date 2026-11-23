# Lane W4 (oltp), 2026-11-23 23:07Z to 00:07Z: two flushers and a failed flush; W3's A/B on Mac Studio 2

Ada's goal: Amphora about 10x faster than every competitor, with no loss of durability. Lane W3
removed the flusher's timed gathering window (81fc0dca, on main in 6a3e8ebb) and left two things
open: a possible durability hole with more than one flusher, found by reading the code, and an A/B
too noisy at load 20 to 44 on adacachy2 to say whether update_plain lost 11% at 32 clients.

## Verdict, losses first

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
