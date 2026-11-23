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

| arm | runs | ops/s median (range) | p50 ms median (range) | flush ms a flush | checkpoint copies |
|---|---|---|---|---|---|
| A main 5f511d8d | | | | | |
| B e57a028b | | | | | |
| SQLite | | | | | |
