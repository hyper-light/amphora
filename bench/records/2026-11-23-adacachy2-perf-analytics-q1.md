# Lane Q1: why table_count trails Turso in S8's suite run (adacachy2, 2026-11-23)

Deliverable status, losses first. The cause is found from a trace and a profile; the fix is not
built and the A/B is not run (the hour ended at the cause). What S8 measured as "table_count" is not
the cost of a count: it is the cost of a count whose reader keeps none of the pages it reads, a state
the statements S8 ran before it leave behind. On a database in its settled state the same count runs
at 0.69-0.83x of Turso's throughput, not 0.06x.

## The statement and its plan

`SELECT COUNT(*) FROM items`, 200,000 rows. All three engines choose the same plan, `SCAN items USING
COVERING INDEX items_owner`. Amphora's bytecode is `OpenRead` on items_owner (root 31833), then one
`Count`: it counts the cells of each leaf page, as SQLite's OP_Count does, and decodes no row. The
index is 531 pages (528 leaves, 200,000 cells; `dbstat` on the SQLite copy of the same data). The
planner's estimate is 200,000 rows, the actual count is 200,000.

## Settled database: the count is near Turso

Binary: S7's 795faa83, the one S8's numbers came from (S8's 701486af changed shell only). kiln run
r1691, adacachy2, `--reuse` (the database reopened after its load), `--only 'table_count$'`, 1 s
window after 0.5 s warm-up, one run each, load 13-36 on 32 cores.

| clients | Amphora | Turso | SQLite | Amphora / Turso |
|---|---|---|---|---|
| 1 | 36,067/s, p50 0.026 ms | 43,409/s, p50 0.022 ms | 8,951/s, p50 0.107 ms | 0.83x |
| 32 | 522,701/s, p50 0.038 ms | 762,430/s, p50 0.030 ms | 2,512/s, p50 12.2 ms | 0.69x |
| 128 | 472,735/s, p50 0.041 ms | 627,998/s, p50 0.030 ms | 2,155/s, p50 56.8 ms | 0.75x |

Read syscalls in this state: 646 in the whole 1-client phase (warm-up included), about 0.03 a
count at 32 clients (23,250 for ~786,000 counts). The pages stay in the reader's cache.

## S8's state: every count reads every page from the file

S8 and S6 ran table_count after point_pk, point_secondary, point_text, filter_no_index,
aggregates_range, join_three, json_field, blob_read and table_group_by, in one process. Run r1708
repeats that order (fresh load, then those reads, then table_count) with every thread of the process
sampled from /proc (`q1_sample.py`):

| table_count phase | read syscalls | bytes read | bytes / read |
|---|---|---|---|
| 1 client (1.52 s) | 1,561,786 | 6,397,027,006 | 4,096 |
| 32 clients (1.62 s) | 4,910,842 | 20,114,760,384 | 4,096 |

At 531 pages a count, 1,561,786 reads is about 2,940 counts in the 1.5 s phase, about 1,960 a
second (inferred from the syscall count; S8 measured 2,220/s and S6 2,183/s in this cell). Each count
reads all 531 index pages with `pread`, one 4 KiB page at a time.

Profile of the 1-client window in that state (AMPHORA_PROFILE, 913 samples, inclusive): `op_count`
86%, `read_page` 81%, `read_page_uncached` 57%, `read_page_no_cache` 52%, `begin_read_page` 39%,
the read's completion callback (`OnceLock` init per completion) 32%, `cache_insert` 13% of which
`make_room_for` (an eviction per insert) 9%, xxh3 over each page (`impl_241_plus_bytes`) 12%,
`find_frame` 6%. The `pread` itself is in libc, which the profiler's blocklist drops, so its kernel
time is under-counted here; the syscall counts above carry it. In the settled state the same profile
is `read_page` 45% with cache hits only (`read_page_uncached` absent).

Where clients wait at 32 and 128 clients in S8's state (thread states, per phase): at 32 clients
79.6% of samples running or runnable, 5.2% in `futex_wait`; at 128 clients 65.4% running, 27.6% in
`futex_wait`. The 32-client profile's leaves include `_mm_pause` 7.1% (a spin wait) beside atomics
(`atomic_sub` 18%, `atomic_load` 10%). So at 32 clients the clients are busy re-reading pages; at 128
a quarter of the samples are waiting on a lock. Which lock is not traced yet.

## Which statement leaves the state

| run | sequence before table_count | table_count 1-client read syscalls |
|---|---|---|
| r1691 | none (reopened database) | 646 |
| r1701 | table_group_by (all three client counts), reopened database | 1,911 |
| r1713 | S8's nine reads, reopened database | 1,650,474 |
| r1713 | none, fresh load in the same process | 1,011 |
| r1708 | S8's nine reads, fresh load | 1,561,786 |

The fresh load does not cause it, and a full scan alone (table_group_by) does not either. One of
point_pk, point_secondary, point_text, filter_no_index, aggregates_range, join_three, json_field and
blob_read, or their combination with the scans, leaves the readers' page caches in a state where a
count's 531 pages are evicted before the next count reads them.

## What is named, and what is not

- Named, from the syscall counts and the profile: the limiter of S8's table_count cell is the reader's
  page cache missing every page of the count, so each count pays 531 `pread` calls, a buffer
  allocation, an xxh3 checksum and a cache insert with an eviction per page.
- Not named yet: why the cache evicts them. The cache is SIEVE with reference levels
  (`page_cache.rs`, `evict_one`), 2,000 pages a reader (`READER_PAGES`); the count's 531 pages fit.
  The next step is the cache's own numbers in this state (capacity, length, evictions, and the
  reference levels of what it keeps), then bisecting the nine statements. Until those are measured,
  any account of the eviction is a guess.
- Answers: the `--query` runs answered 200,000 on Amphora and on SQLite; S8's answer check had already
  found Amphora's and Turso's answers equal to SQLite's.

## Commands

All on adacachy2 (runner 00aad8daff2fa3c9daa65d53c425d450), binary
`~/.cache/amphora-perf-bin/bench-s7/amphora-bench-795faa8377d79ffe4d82f3eb7cb19717453c3a9e`, scripts in
`2026-11-23-adacachy2-perf-analytics-q1/`:
- r1691: `q1_diag.sh` (plans, bytecode via `--query --explain`, `dbstat`, profiles, the trace, the
  reference table above), needs `linux/amd64,cpus=2,mem=8g`.
- r1701: `q1_repro.sh` with `ONLY='table_group_by$,table_count$'`.
- r1708: `AMPHORA_PROFILE=… python3 q1_sample.py trace3.json amphora-bench --suite --items 200000
  --target amphora --dir … --clients 1,32,128 --seconds 1 --warmup 0.5 --only
  'point_pk$,point_secondary$,point_text$,filter_no_index$,aggregates_range$,join_three$,json_field$,blob_read$,table_group_by$,table_count$'`.
- r1713: the same with `--reuse`, then a fresh load with `--only 'table_count$'`.

The host was shared throughout (1-minute load 13 to 79 on 32 cores, my own build of main 4536a0d4
running beside r1701 to r1713), so every throughput above is one run and directional. The syscall
counts and the profile shares do not depend on the load.
