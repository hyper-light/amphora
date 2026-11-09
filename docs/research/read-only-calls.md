# Read-only calls and the flush: what a session that wrote nothing waits for

Source note for docs/design/store.md decision 32 (kiln goal f4b650c4, research
01a194f7c72372488edea1c8e6443ee7).

## The report

kiln's `ci::work::tests::w24_ac10_lookup_p99_under_5ms` (W24-AC10: lookup p99 under 5 ms) on
Amphora 29be700, at kiln 36ec7e77fa on adacachy at load 40-90, took about 92 ms a lookup: index 4306
got UnknownOutcome after about 400 s. code_mode's `w20_ac02` ended 52 of 100 trivial CI runs in 18
minutes. Both pass on kiln's SQLite main. The report read this as read-only `Database::call`s
waiting for a fresh fsync.

## What a read-only call does (29be700)

- `Database::call` submits the session as one batch through the store's `put_then` and answers in
  its durable callback (`database.rs`, `call_db`, `submit`).
- The engine runs the session in its own transaction. A commit with no dirty pages writes no log
  frames (`pager.rs`, `commit_tx`), and under `PRAGMA synchronous = NORMAL` it syncs nothing.
- The batch's ticket waits for a flush, which the flusher starts for any settled batch
  (`store.rs`). The flush is `Db::flusher`, which returns before syncing when the log's prefix
  is already durable (`db.rs`, `prefix.is_durable()`).

## Measured

The probe, a scratch test never committed, on adas-mac-studio-3 (macOS, aarch64, where a flush is
F_FULLFSYNC), 29be700, 2026-11-01: `Database::open`, 20,000 rows in one call, then 3 rounds of 500
sequential SELECT-only calls.

| Round | Log bytes before → after | p50 | p99 | max |
|---|---|---|---|---|
| 0 | 791,072 → 791,072 | 78.8 µs | 721.2 µs | 1.46 ms |
| 1 | 791,072 → 791,072 | 77.8 µs | 127.3 µs | 3.61 ms |
| 2 | 791,072 → 791,072 | 76.6 µs | 91.5 µs | 97.0 µs |

## Why kiln's lookups were slow

kiln's `judge()` (`crates/kiln/src/ci/work.rs` at 36ec7e77fa, line 674) calls
`bump_hits(c, scope, &strong)` on every hit. So each W24-AC10 lookup is a write, and a write is
acknowledged only once a flush that began after its commit ends (decision 15; AC-T1.4.4). On
adacachy's loaded btrfs that flush is tens to hundreds of milliseconds. SQLite in NORMAL answers at
commit, before any sync, which is why kiln's main passes.

## Elsewhere

- PostgreSQL `xact.c` (REL_18_0), `RecordTransactionCommit`: "If we haven't been assigned an XID
  yet, we neither can, nor do we want to write a COMMIT record."
- SQLite `pager.c` (3.54.1), `sqlite3PagerCommitPhaseOne`: "If no database changes have been made,
  return early."
- Kimura, Graefe and Kuno, ADMS 2012, §5.3: a read-only transaction "immediately exits if the
  maximum tag is already durable. Otherwise, it wakes up the log flusher and waits until the LSN
  becomes durable."
- Engines that answer at commit trade away what a power cut can take back: PostgreSQL 18 docs on
  `synchronous_commit`; SQL Server's delayed durability; SQLite's `PRAGMA synchronous`, "A
  transaction committed in WAL mode with synchronous=NORMAL might roll back following a power loss
  or system crash."
