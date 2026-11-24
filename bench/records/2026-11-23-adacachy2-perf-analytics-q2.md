# Lane Q2: why table_count re-reads its index after point lookups (adacachy2, 2026-11-23)

Deliverable status, losses first.

- The A/B in S8's order is not run. `q2_ab.sh` is written for it: before and after alternated, with
  Turso and SQLite in every round and the load recorded.
- Merged to main: the fix (8574888e), its test (f3100b2c) and this record. The merge gate passed on
  adacachy2. At f3100b2c (r1788), `storage::page_cache` passed 44, failed 0, ignored 1, and clippy on
  amphora-engine `--all-targets -D warnings` exited 0. On the tree merged with main (dbe0200b, r1793)
  the same results held.
- The test fails on the old code. With the test alone on main's f0fb3688 (0c90521f, r1791), the file
  reads per count were `[51, 50, 50, 50]`: every count re-read its index.
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
  `cargo clippy -p amphora-engine --all-targets --locked -- -D warnings` at f3100b2c; r1793 the same
  at the merge dbe0200b; r1791 the new test alone at 0c90521f (old cache code).
- Next: `q2_ab.sh <main before> <main after>`, with both binaries built by `q2_census.sh`'s build
  step or Q1's `q1_build.sh`.

## The A/B (r1798, added by lane Q3)

`q2_ab.sh b6ab0946 c6f915b1`: A is main before the fix, B is main with it (c6f915b1 holds
8574888e). Five rounds alternated (A,B then B,A), S8's ten statements in S8's order at 1, 32 and 128
clients, 1 s after 0.5 s of warm-up, on Q1's database (`--reuse`), Turso and SQLite in every round.
Statements a second, median of 5 (range):

| statement | clients | A (b6ab0946) | B (c6f915b1, the fix) | B / A | Turso | SQLite | B / best competitor |
|---|---|---|---|---|---|---|---|
| point_pk | 1 | 246,218 (146,682-284,541) | 254,379 (156,732-277,276) | 1.03x | 440,616 (385,534-519,752) | 394,073 (260,694-449,294) | 0.58x of turso |
| point_pk | 32 | 764,188 (599,798-797,102) | 773,523 (463,185-801,376) | 1.01x | 1,110,583 (1,010,851-1,241,604) | 227,158 (215,859-248,808) | 0.70x of turso |
| point_pk | 128 | 625,039 (410,586-690,289) | 627,960 (450,420-690,692) | 1.00x | 694,934 (623,525-765,914) | 145,926 (120,908-170,096) | 0.90x of turso |
| point_secondary | 1 | 205,651 (166,630-227,715) | 196,010 (12,326-212,490) | 0.95x | 298,609 (287,148-382,936) | 164,674 (118,873-204,439) | 0.66x of turso |
| point_secondary | 32 | 621,888 (512,036-783,084) | 654,836 (268,234-723,965) | 1.05x | 1,044,838 (995,811-1,230,869) | 89,315 (51,749-110,677) | 0.63x of turso |
| point_secondary | 128 | 476,722 (242,143-516,804) | 472,694 (261,087-490,548) | 0.99x | 379,517 (353,454-523,824) | 58,596 (39,946-68,335) | 1.25x of turso |
| point_text | 1 | 191,681 (151,989-219,579) | 178,905 (120,732-197,486) | 0.93x | 277,999 (266,992-345,066) | 158,758 (119,504-214,989) | 0.64x of turso |
| point_text | 32 | 677,311 (554,994-793,591) | 634,880 (293,305-685,876) | 0.94x | 1,058,689 (992,230-1,193,572) | 76,385 (58,102-113,559) | 0.60x of turso |
| point_text | 128 | 424,789 (219,253-455,845) | 452,768 (220,998-472,529) | 1.07x | 348,254 (329,711-376,037) | 65,246 (46,085-85,809) | 1.30x of turso |
| filter_no_index | 1 | 22 (16-26) | 18 (12-26) | 0.82x | 30 (15-35) | 36 (32-52) | 0.50x of sqlite |
| filter_no_index | 32 | 121 (76-148) | 57 (43-93) | 0.47x | 153 (64-178) | 32 (10-32) | 0.37x of turso |
| filter_no_index | 128 | 0 (0-2) | 0 (0-0) | - | 14 (0-17) | 0 (0-2) | 0.00x of turso |
| aggregates_range | 1 | 3,282 (2,513-3,803) | 2,928 (2,413-3,758) | 0.89x | 4,539 (1,762-5,360) | 6,087 (1,710-7,459) | 0.48x of sqlite |
| aggregates_range | 32 | 21,687 (10,898-28,944) | 13,288 (9,235-15,021) | 0.61x | 37,528 (30,995-39,617) | 4,647 (4,273-5,078) | 0.35x of turso |
| aggregates_range | 128 | 9,823 (7,065-14,097) | 8,707 (3,945-9,471) | 0.89x | 11,259 (5,090-11,877) | 3,369 (2,433-3,920) | 0.77x of turso |
| join_three | 1 | 429 (291-512) | 495 (388-569) | 1.15x | 68 (18-73) | 1,751 (683-2,129) | 0.28x of sqlite |
| join_three | 32 | 2,488 (1,984-3,815) | 2,370 (1,742-2,633) | 0.95x | 915 (820-1,099) | 985 (942-1,190) | 2.41x of sqlite |
| join_three | 128 | 1,674 (1,377-3,622) | 1,639 (1,402-3,243) | 0.98x | 832 (462-982) | 756 (702-817) | 1.97x of turso |
| json_field | 1 | 184,409 (112,666-260,460) | 218,756 (136,711-237,338) | 1.19x | 400,653 (112,396-450,740) | 371,672 (116,006-489,249) | 0.55x of turso |
| json_field | 32 | 579,237 (277,047-837,948) | 760,184 (337,053-784,638) | 1.31x | 1,176,804 (1,045,105-1,294,990) | 235,874 (209,996-251,618) | 0.65x of turso |
| json_field | 128 | 430,600 (227,116-690,154) | 557,767 (199,579-668,442) | 1.30x | 380,020 (265,866-403,495) | 157,692 (123,670-177,018) | 1.47x of turso |
| blob_read | 1 | 224,834 (134,445-292,679) | 220,431 (153,081-287,810) | 0.98x | 474,058 (173,342-570,660) | 522,343 (319,434-572,161) | 0.42x of sqlite |
| blob_read | 32 | 620,995 (319,889-859,737) | 744,994 (361,201-794,346) | 1.20x | 1,177,572 (1,014,866-1,240,392) | 244,115 (222,726-261,117) | 0.63x of turso |
| blob_read | 128 | 569,281 (271,622-751,834) | 647,325 (203,395-705,551) | 1.14x | 358,202 (321,534-419,789) | 174,409 (152,001-185,502) | 1.81x of turso |
| table_group_by | 1 | 14 (8-16) | 11 (9-17) | 0.79x | 4 (2-5) | 6 (4-7) | 1.83x of sqlite |
| table_group_by | 32 | 113 (59-162) | 64 (32-64) | 0.57x | 12 (3-22) | 0 (0-0) | 5.33x of turso |
| table_group_by | 128 | 0 (0-7) | 0 (0-0) | - | 0 (0-0) | 0 (0-0) | - |
| table_count | 1 | 2,001 (1,460-2,158) | 32,065 (23,484-34,561) | 16.02x | 37,211 (36,298-41,724) | 7,547 (5,894-8,473) | 0.86x of turso |
| table_count | 32 | 26,087 (7,711-54,938) | 281,960 (122,843-339,407) | 10.81x | 545,032 (458,178-676,433) | 3,112 (2,430-11,824) | 0.52x of turso |
| table_count | 128 | 21,782 (17,064-32,811) | 243,345 (98,482-308,279) | 11.17x | 489,667 (392,985-560,221) | 2,063 (1,926-7,139) | 0.50x of turso |

Verdict: table_count is faster, 16.0x at 1 client, 10.8x at 32 and 11.2x at 128 (medians; the ranges
don't overlap at any client count: A's highest 2,158 / 54,938 / 32,811 against B's lowest 23,484 /
122,843 / 98,482). B against Turso there: 0.86x, 0.52x, 0.50x.

Q2's guess, that point reads right after a full scan may now find fewer pages cached: r1798 does not
show it. In this order json_field and blob_read run right after filter_no_index, aggregates_range and
join_three. B over A for json_field is 1.19x, 1.31x, 1.30x and for blob_read 0.98x, 1.20x, 1.14x, and
every range overlaps: no measurable difference, and no loss. r1798 counts statements, not cache hits,
so it cannot rule out fewer cached pages that cost too little to show; Q2's census would count them.

One line, not followed (not this lane's): the full scans themselves ran slower under B at 32 clients
(filter_no_index 0.47x, table_group_by 0.57x, aggregates_range 0.61x; the ranges overlap except
table_group_by's, A 59-162 against B 32-64). A rerun of those alone would say if it is real.

`q2_ab.sh`'s summary does not count errors; the answers were not checked in this run.

The host was shared: the 1-minute load before each run, with CPU pressure (some, avg10):
calib-A 30.3 (9.4%), calib-B 34.2 (44.3%); A-1 68.8 (47.4%), B-1 72.9 (47.1%), turso-1 75.2 (49.3%),
sqlite-1 98.6 (71.6%); B-2 100.1 (37.5%), A-2 89.7 (42.7%), turso-2 63.9 (40.6%), sqlite-2 106.2
(75.1%); A-3 87.6 (37.3%), B-3 100.0 (50.8%), turso-3 95.7 (46.2%), sqlite-3 85.4 (67.8%); B-4 84.9
(43.0%), A-4 60.7 (47.3%), turso-4 84.3 (46.1%), sqlite-4 122.1 (70.8%); A-5 109.0 (44.5%), B-5
131.7 (68.5%), turso-5 118.4 (65.2%), sqlite-5 140.5 (78.1%); end 106.3. Lane Q3's own runs shared
it: r1813 and r1819 (00:11-00:13Z, rounds 1's A to sqlite), r1887 (00:27-00:33Z, rounds 4 and 5)
and r1903 (from 00:31Z, round 5). The runs are in `~/.cache/analytics-q2/ab/runs.jsonl` on adacachy2.
