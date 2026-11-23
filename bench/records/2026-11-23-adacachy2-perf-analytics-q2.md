# Lane Q2: why table_count re-reads its index after point lookups (adacachy2, 2026-11-23)

Deliverable status, losses first.

- Not merged. The fix (8574888e) and its test (f3100b2c) are on `perf/analytics`. The merge gate
  (clippy on amphora-engine, the `storage::page_cache` tests) is run r1788 on adacachy2, with r1780
  queued on macOS. The hour ended before either was read.
- The A/B in S8's order is not run. `q2_ab.sh` is written for it: before and after alternated, with
  Turso and SQLite in every round and the load recorded.
- The new test has not been run against the old code, so it has not been shown to fail there.
- Measured and done: the cause, from the reader cache's own numbers, and the fix's effect on those
  numbers in the real run (below).

## How it was measured

The diag branch `perf/analytics-q2-diag` (e22de66b, not for main) adds a census to each reader's
page cache: capacity, length, inserts, evictions, the hand's place in the queue, and for each kept
page its mark (0 to 3), its hits and when it joined. `AMPHORA_CACHE_CENSUS=<dir>` writes one census
before and one after each of a reader's first four counts. `q2_census.sh` builds the bench at a SHA
and runs `--only 'point_secondary$,table_count$'` at 1, 32 and 128 clients on Q1's database
(`--reuse`, 200,000 items). `q2_census.py` prints each reader's census and `q2_agg.py` sums them over
every reader. In Amphora's file the count reads 607 pages of `items_owner`. Q1's 531 came from
`dbstat` on the SQLite copy.

## The cause

Over all 128 readers, here are the counts that ran on a full cache after point_secondary. The first
of them reads its pages. The rest should read none.

| build | readers whose later counts read the file | later counts that read the file | pages they read | hand to head at the first count (min / median / max) |
|---|---|---|---|---|
| main + census (e22de66b), r1752 | 32 of 128 | 95 of 383 | 57,564 | 143 / 1,330 / 1,601 |
| fix + census (44995423), r1773 | 0 of 128 | 0 of 383 | 0 | 2 / 574 / 1,996 |

On the worst reader before the fix (census 55d9e0c35260, hits up to 56,289 on one page):

| count | file reads | evicted (older pages / its own) | hand | ahead of the hand (marks 0/1/2/3) | behind the hand (marks 0/1/2/3) |
|---|---|---|---|---|---|
| 1 | 607 | 143 / 464 | 1,857 | 143 (143/0/0/0) | 1,857 (254/254/177/1,172) |
| 2 | 603 | 0 / 603 | 1,861 | 139, all its own | 1,861 (258/254/177/1,172) |
| 3 | 603 | 0 / 603 | 1,861 | 139, all its own | 1,861 (254/258/177/1,172) |

A new page joined the queue at its head (the back). The hand sweeps toward the head, so it reached
a new page after only the pages between itself and the head. Point lookups had left the hand 143
pages from the head. The count's first 143 misses evicted those 143 unmarked pages. Its 144th miss
evicted the count's own first page. That page was read moments before, was unmarked, and would be
read again by the next count. Meanwhile 1,857 pages lay behind the hand, 254 of them unmarked. That
144th eviction is the first wrong decision. From then on, each miss evicted the count's next own
page. Each eviction was matched by an insert at the head, so the hand never reached the head and
never returned to the pages behind it. Their marks are unchanged across counts 1 to 3. The cache
worked as 139 slots for a 607-page loop, and every count read 603 to 607 pages from the file.

The other 96 readers' later counts read nothing from the file. This run did not measure how many
unmarked pages each of them had ahead of its hand.

## The fix

`page_cache.rs`, `_insert`: a new page joins just behind the hand, where the sweep looks last. The
hand reaches it after every page already cached. This is what decision 19 says the cache does
(`docs/design/store.md`, updated). Insertion at the head contradicted it whenever the hand was past
the tail. In the same run, no reader's later count read the file (table above).

A variant tried beside it starts a new page with one mark (76e70d9f, r1773). It also read nothing on
later counts (0 of 383), so it adds nothing here, and it is not landed.

`q2_counts_after_point_lookups_read_no_page_from_the_file` (page_cache.rs) runs 20,000 point lookups
through one index of a 20,000-row table larger than a 200-page cache, then four counts through
another index. It asserts each count answers 20,000 and the counts after the first read no page from
the file (`Pager::page_file_reads`).

## Numbers not to cite as an A/B

The census runs used diag binaries, one run each, with an extra atomic per cache hit and the census
writes inside the timed window, at load 20 to 52 on 32 cores. table_count ops/s at 1/32/128 clients:
main + census 26,216 / 138,570 / 83,133. Fix + census 24,357 / 257,019 / 275,399. One-line note, not
followed: in this two-statement order the 1-client table_count already ran at about 26k/s before the
fix, against S8's 2,220/s in its ten-statement order.

## Commands

All on adacachy2 (runner 00aad8daff2fa3c9daa65d53c425d450), from the GitHub clone
`~/.cache/amphora-perf/analytics-remote` (sync false), scripts in
`2026-11-23-adacachy2-perf-analytics-q2/`:
- r1752: `q2_census.sh e22de66b` (needs linux/amd64,cpus=8,mem=24g).
- r1773: `JOBS=4 q2_census.sh 44995423`, then `76e70d9f` (cpus=4,mem=12g after a refusal at 8).
- `q2_agg.py` over the three census folders under `~/.cache/analytics-q2/`.
- r1788: `cargo test -p amphora-engine --lib storage::page_cache` and
  `cargo clippy -p amphora-engine --all-targets --locked -- -D warnings` at f3100b2c.
- Next: `q2_ab.sh <main before> <main after>`, with both binaries built by `q2_census.sh`'s build
  step or Q1's `q1_build.sh`.
