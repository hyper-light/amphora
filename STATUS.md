# Status

Updated 2026-10-20. A piece is done when its tests pass in the gate (`scripts/gates.sh`: Linux, with the
macOS and Windows lints) and the evidence its requirement names is recorded.

## Implemented

- **The bar** (kiln's CP-M1): a Cargo workspace whose lints deny panics, this repository's rules
  (`CLAUDE.md`), the requirement registry (`docs/requirements.toml`), the design's starting point
  (`docs/design/README.md`), the gate (`scripts/gates.sh`, run by kiln CI from `.kiln/ci.yaml`), and the
  vendoring tool (`scripts/vendor.py`, kiln's).
- **The `amphora` binary** says what it is (`amphora --version`, `amphora --help`), and nothing more yet.
- **The store's commit pipeline** (CP-T1.1, `crates/amphora/src/store.rs`, docs/design/store.md): one
  writer with a group commit and no timer; durable beside it by a flusher (a write is durable once a
  flush begun after its commit ends, `Store::durable` by its batch's ticket); readers of their own on
  their caller's thread, each waiting only for its key's writes; no checkpoint in a commit's way (the
  log copied until little is left before the writer starts it again). A full queue refuses rows,
  typed and counted, and `put_wait` waits for room. A commit that fails is retried batch by batch:
  a batch that can't be committed is refused to its sender (`Refused::Failed`), the rest kept. The
  writer keeps no statement journal. Its meter keeps the longest commit, flush, checkpoint, wait to
  restart and restart. A flush that fails is the last: the store refuses what waits and what comes
  (`Refused::Disk`) until it is started again. The flusher flushes the log's data, and the log keeps
  its length across restarts (decision 13); when the writer ran out of work during a flush, the next
  waits for it to take in what that flush's senders send (decision 14): at 32 writers on fedora,
  7,605 writes a second at p50 4.06 ms against 5,608 at 5.12 ms before, and PostgreSQL 17's 6,120 at
  5.06 ms in the same windows (2026-10-20, load 4–12, not yet a quiet machine). Tested in
  `crates/amphora/tests/store.rs`.
- **Groups** (`crates/amphora/src/groups.rs`, docs/design/store.md decision 10): a store in groups
  on one node, keys spread by hash, each group a writer, a log and a flusher of its own; a batch is
  one group's. 16 groups wrote 320,451 rows a second at 1,000,000 writers on fedora, against one
  writer's 22,000. `Store::put_then` tells a sender once its batch is durable (decision 11).
- **Crash injection** (T-T1.1.1, `crates/amphora/tests/crash.rs`, docs/design/store.md): the store on
  a simulated disk that loses its power at any change, unflushed writes surviving whole, torn or not
  at all, its flushes taking time and covering what was written before they began, batches left in
  flight so commits land during flushes. 10,000 seeds: no acknowledged batch lost, no refused batch
  kept, no batch torn, the integrity check passing (2026-10-20). Every gate runs 200. A test holds a
  flush and commits during it: no checkpoint copies that commit before a later flush (decision 15).
- **Stores on the engine** (`crates/amphora/src/db.rs`): a database opened for the pipeline, its
  readers, its flusher, its checkpoints and its log's restart.
- **Amphora's engine** (`crates/engine*`, its `ORIGIN.md`): Turso's four crates (core, sqlite/parser,
  macros, extensions/core) as of 3e09a82, Amphora's own code since 2026-10-20 as `amphora-engine`,
  `-parser`, `-macros` and `-ext`, members of the workspace, nothing compiled from C. Its suites run
  in every gate; what is left of the inherited code that breaks CLAUDE.md §1 is counted, lint by lint,
  against `scripts/engine-ratchet.toml`, and may only fall. The bench's vanilla Turso is upstream at
  3e09a82, none of Amphora's changes.
- **The packed log** (`crates/engine/storage/packed.rs`, docs/design/store.md decisions 16 and 17),
  opt-in (`Db::pack_log`): a packed log holds records, not a page a frame: a commit's pages as whole images, or
  as the bytes that changed since their last version in the log (at most 8 deltas in a row),
  compressed together with LZ4 into the log's frame slots. Reads rebuild a page from its chain, on a
  fresh connection, in the checkpoint and in recovery alike; a record torn by a crash ends the log,
  one whose checksums hold but that doesn't decode refuses it. Crash injection, 10,000 power cuts on
  the committed tree: nothing lost (2026-10-20); the engine's tests include a model test of
  savepoints rolled back amid spills. On the bench so far it cost writes at 32 writers (6,216 and
  5,887 a second against 7,903, `bench/records/2026-10-20-fedora-packed-log.jsonl`), so it isn't the
  default until the comprehensive bench shows where it helps. The comprehensive bench shows where it
  hurts: statements that write hundreds of pages, `insert_batch_100` at 32 clients 392 → 44 a
  second, `delete_range_insert` 2,800 → 1,011 (fedora, 2026-10-22, checks); single-row writes as
  before.
- **The planner where the suite found it choosing far more work** (docs/design/planner.md,
  `crates/engine/tests/planner.rs`): a closed range with unknown bounds is 0.5% of its table whether
  it drives a seek or filters one, so a join from a key range looks each row's partner up by key; a
  sort is charged to the access path that needs it, on the rows it sorts, so a page by key reads in
  key order. EXPLAIN words a range on the rowid as one. Checks on fedora, 1 client (not recorded
  runs): `join_pk` 71 → 94,710 statements a second, `join_three` 56 → 1,598, `offset_deep` 25 → 64.
- **A GLOB prefix seeks its index** (docs/design/planner.md decision 3, `crates/engine/tests/glob_range.rs`):
  `col GLOB p` on a TEXT column implies the range of `p`'s literal prefix, computed when the
  statement runs, so an index seeks it; a BLOB matches no GLOB or LIKE (SQLite's recommended
  `SQLITE_LIKE_DOESNT_MATCH_BLOBS`). The suite asks SQLite's dialect for GLOB where it asks the
  others for LIKE, as their LIKE heeds case.
- **LIKE jumps to its literal** (docs/design/executor.md decision 3): past a `%`, an ASCII pattern's
  next literal is found with `memchr2` in either case, as SQLite's `patternCompare` finds it with
  `strcspn`. `like_substring` 67 → 222 statements a second at 1 client and 867 → 2,556 at 32
  (fedora, best of 3 at load under 2, not recorded runs).
- **Table lookups cost what the cache can't hold** (docs/design/planner.md decision 5): a row read
  through an index that lacks its columns costs a page read by Mackert and Lohman's finite-buffer
  formula, with the connection's own cache (and shared pages), as PostgreSQL's
  `index_pages_fetched` counts them. Of the suite's 50 plans only `table_group_by` changes: it scans
  the table and sorts instead of looking every row up through `items_grp_created`: 4.0 → 14.2
  statements a second at 1 client and 35.2 → 167.5 at 32 (fedora, best of 2 at load under 2, not
  recorded runs).
- **Many searches of a cached index read its pages once** (docs/design/planner.md decision 6): a
  nested loop's searches of an index the connection's cache holds whole are charged its pages
  once at most (Mackert and Lohman, as PostgreSQL charges an index read over many loops). Of the
  suite's 50 plans only `cte` changes: its grouped rows drive lookups of `owners` by key, instead of
  every owner probing an index built on them. `cte` 218 → 324 statements a second at 1 client and
  2,372 → 2,904 at 32 (fedora, best of 3 at load under 2, not recorded runs).
- **Skip-scan** (docs/design/planner.md decision 4, `crates/engine/tests/skip_scan.rs`): a constraint
  on an index's second column alone seeks it once for each value of its first, where ANALYZE
  counted at least 18 rows to a value, as SQLite does. `count_range` 165 → 2,476 statements a
  second at 1 client and 2,644 → 32,892 at 32 (fedora, best of 3 at load 2–7, not recorded runs).
- **Readers keep their pages across commits** (docs/design/store.md decision 18,
  `crates/engine/storage/changes.rs`): a connection starting a read transaction forgets only the
  pages written since its last one, where SQLite and Turso forget its whole cache; it forgets
  everything when the log can't say (another generation, a gap, more pages than it caches). A model
  test of random transactions, checkpoints and restarts, both log formats, never reads a stale row,
  and catches a reader that forgets nothing. With it, readers keep 2,000 pages (decision 5): K0's
  reads are as fast as at 128, the suite's at 32 clients up to 62 times faster, for 8 MiB a reader.
- **The page cache evicts as SIEVE does** (docs/design/store.md decision 19): a new page joins the
  queue's head, so the hand reaches it after every older page; inserted after the hand, as Turso
  had it, a new working set never settled in a full cache (`order_by_index` after
  `point_secondary`, 2,000 pages: 49,044 → 147,133 a second, a check on fedora at 1 client).
- **Shared pages** (docs/design/store.md decision 20, `crates/engine/storage/shared_pages.rs`), opt-in
  (`Db::share_pages`): a page one connection read in, every connection reads from memory, keyed by
  the version it is; a writer copies a shared buffer first, a file write drops the file's version.
  Sharing 256 MiB, the suite's analytic reads run 2–2.6 times as fast at 1 and 32 clients and K0's
  reads 43% faster, for K0's resident set 340 MB against 83 and 5% fewer writes. One pool now serves
  a process's databases (`Db::share_pages_in`), each database's pages under keys of its own; run
  past the suite's first seconds, every read measured is faster shared (point reads 1.33–1.49 times
  at one client), with 32 readers for less memory. Not on by default: that, and its budget, are
  the person's call.
- **A shared page found in one lookup** (docs/design/store.md decision 22): the pool keeps each image
  in its map's entry and counts lookups in each shard, under the lock it holds anyway, where one
  count for the pool moved between 32 readers' cores at every lookup. Sharing 256 MiB at 32 clients:
  `order_by_sort` 8,566 → 11,660 statements a second, `exists_subquery` 11,330 → 15,483,
  `group_by_having` 8,169 → 9,851 (fedora, best of 2 at load under 2, not recorded runs).
- **A page read records itself only when it must wait** (docs/design/store.md decision 21): a page
  read from the file goes straight into the cache, recorded for a resumed call only when a spill
  holds it up; every read had taken two write locks and a hash insert and remove. Whole scans
  5–7% faster (a probe of the engine on fedora, checks).
- **A commit ends before its pages outgrow the writer's cache** (docs/design/store.md decision 23):
  the writer takes whole batches into a commit until the next might make its cache spill written
  pages into the log, and commits the rest at once after (`Engine::apply_batches`,
  `Db::transaction_while_room`); statements of a hundred rows had made commits of 3,400 pages
  through a 1,024-page cache. At 32 clients `insert_batch_100` 333 → 456 statements a second (p99
  203 → 131 ms), `delete_range_insert` 2,841 → 3,136, peak resident 102 → 59 MB (fedora, checks).
- **Ephemeral tables kept between runs** (docs/design/executor.md decision 1,
  `crates/engine/tests/ephemeral_reuse.rs`): a statement keeps each ephemeral table of at most 16
  pages (an IN list's keys, an IN subquery's rows) and empties it on its next run, where building
  one each run (a temp file, a pager, a transaction, a B-tree) was a third of `in_list_10`'s time.
- **`IN (SELECT ...)` searches its list once a row** (docs/design/executor.md decision 2,
  `crates/engine/tests/in_subquery.rs`): as a condition a miss fails the row at once; as a value or
  under `NOT IN`, a one-column list's first entry says whether it holds a NULL, where every miss
  read the whole list. `subquery_in` 213 → 784 statements a second at 1 client, 2,846 → 6,833 at
  32 (fedora, best of 3 in turns, not recorded runs).

## Measured

- **Against every competitor, 32 to 1,000,000 writers** (`bench/records/2026-10-20-fedora-matrix.jsonl`,
  `python3 bench/table.py`; fedora, one run each, load 1.4–66 with other work on the machine, so not
  yet the quiet best of 3 CP-T1.2 asks). Amphora's best (one group at 32 writers, 12 above) against
  PostgreSQL 17: 1.2× the writes at 32 writers, 3.6× at 1,000, 5.8× at 10,000, 3.9× at 100,000,
  7.4× at 1,000,000; p99 1.1×, 2.5×, 1.9×, 1.6×, 7.6×; memory 8.9× at 32 writers, 1.3–2.6× with
  12 groups. Against OrioleDB much the same; against MySQL, Vitess, ClickHouse, vanilla Turso and
  SQLite 3.4–44× the writes at 32 writers and 18–1,128× from 1,000 up. What bounds it: at low
  concurrency the disk's flush (2.8 ms on fedora's btrfs, however the log is written); at high, the
  log's volume (whole pages, about 7 KiB a 200-byte event at 10,000 writers, 1.7 GB/s, as much
  again copied into the file). CedarDB refused the schema's `COLLATE "C"` (the bench now leaves it
  out for CedarDB); MySQL's server exited at 1,000,000 writers and the bench waited on it.

## Next

- **One thread making everything durable**: on btrfs every flush is a commit of the filesystem's
  log, one at a time, so the checkpointer's flushes of the file hold up the flusher's, and with it
  every write (docs/design/store.md, decision 4: commits stalled up to 97 ms, writes at 512 writers
  lost a third of their rate). The checkpoint now copies only frames the flusher made durable and
  flushes no log of its own (decision 15); a pass still flushes the file, 10.9–12.2 ms of each at 32
  writers. Left: the file's flush goes to the flusher's thread, between its flushes of the log,
  before the log starts again. It changes when Turso's checkpoint makes what it copied durable;
  T-T1.1.1's crash injection proves it.
- **CP-T1.1**: kiln vendors Amphora and its conversation store runs on `amphora::store` in place of its one writer thread; the soak bench for drops (AC-T1.1.5) and the stalled-disk test (T-T1.1.2); the bar measured on a quiet machine, best of 3 (AC-T1.1.2–4), with vanilla Turso, SQLite and Vitess beside Amphora (`bench/`).
- **CP-T1.2**: Baselines on a quiet machine against SQLite, Postgres 17, Vitess, CockroachDB and TiDB; the research; the client protocol; `docs/design/` written to it.
- **CP-T1.3**: Turso's MVCC with `BEGIN CONCURRENT` made serializable and production-grade, and the engine multi-writer on it.

## Open

Everything else the track's plan names (kiln plan `01a157fdefba746a94a5fcd920a80d5a`): each goal, its criteria and how each is
measured are in `docs/requirements.toml`.
