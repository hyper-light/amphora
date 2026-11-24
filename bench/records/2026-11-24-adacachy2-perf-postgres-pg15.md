# Lane PG15 (postgres), 2026-11-24: the durable OID counter landed, and the 16385

Hour: 2026-11-24T04:58:27Z to 05:58:27Z (from the lane's first command). Branch
`perf/postgres-pg14`, PG14's, with PG15's commits on it. Not a speed change, so no A/B: the counter
adds no page and no sync to a CREATE TABLE (PG14's message), and nothing here was timed.

## Losses first

- T-PG.7.8's red run on main set the counter through the dialect's private `next_oid` field, because
  main has no `Dialect::load_next_oid`. The branch's own T-PG.7.8 is the one that passed.
- Nothing ran on macOS. The macOS runs (r333, r339, r375) were still queued when adacachy2 had answered, and
  I stopped them.
- Not checked: what a client sees in `pg_class` of CREATE SEQUENCE under the PostgreSQL dialect. A
  sequence's backing table (`__turso_internal_seq_<name>`) is one of the engine's own tables, so it now
  takes no OID either. No PostgreSQL dialect test uses a sequence.
- A rolled-back CREATE TABLE's OID still comes back after a reopen (postgres.md section 7's
  Choices for Ada), as it did before.

## The 16385: a defect, and where it came from

PostgreSQL 19 gives the first relation of a client's CREATE TABLE 16384. `GetNewObjectId` (REL_19_0
src/backend/access/transam/varsup.c:578-605) forces `nextOid` to `FirstNormalObjectId` on its first
call after the postmaster starts and returns it, and `heap_create_with_catalog` takes the relation's
OID first (src/backend/catalog/heap.c:1578), before its array type's (1663) and its row type's
(1674). The tree is meson.build 19.0 at ~/.cache/pg-interrogate-2/pg19.

The trace (r306, adacachy2, probe branch `perf/postgres-pg15-probe` 4c41fbd7: `allocate_oid` printed
each OID and a backtrace, `cargo test -p amphora-engine --lib -- dialect::postgres::tests::t_pg_7_10
--nocapture --test-threads=1`):

| OID given | live OIDs read | The caller |
|---|---|---|
| 16384 | none | `MvStore::initialize_mvcc_metadata_table_nonblock` (CREATE TABLE __turso_internal_mvcc_meta), under `PRAGMA journal_mode = mvcc` |
| 16385 | 16384 | the test's first CREATE TABLE (`kept`) |
| 16386 | 16384, 16385 | `gone` |
| 16387 | 16384, 16385 | `checkpointed`, after the reopen |
| 16388 | 16384, 16385 | `recovered` |

So the metadata table, not a second translation, took the first OID, and its row was in `pg_class`.
The engine's own tables now store no OID: they take none and have no `pg_class` row. T-PG.7.10
asserts exact OIDs again: kept 16384, gone 16385, checkpointed 16386, recovered 16387, and that
`pg_class` has no row for `__turso_internal_mvcc_meta`.

## Red run on main

`perf/postgres-pg15-red` 5ac9083d is main a64549cc with T-PG.7.6, 7.7, 7.8, 7.9 and 7.10. adacachy2,
needs linux/amd64,cpus=2,mem=8g, r390: `cargo test -p amphora-engine --lib --
dialect::postgres::tests::t_pg_7 --test-threads=2`, 4 passed, 4 failed:

| Test | On main | Why |
|---|---|---|
| T-PG.7.6 create, drop, reopen, create | FAILED | left 16384, right 16384: the dropped OID is given again |
| T-PG.7.7 two creates in one transaction | FAILED | after the drop and the reopen, 16385 is `second`'s OID, given again |
| T-PG.7.8 the wrap | FAILED | `SqlError("no OID is left for a new table: OIDs don't wrap around yet")` |
| T-PG.7.10 MVCC checkpoint and recovery | FAILED | `pg_class` shows `__turso_internal_mvcc_meta` |
| T-PG.7.9 rolled-back create | passed | main's counter was already outside the transaction |

## Checks

Each on adacachy2 (linux/amd64, `sync: false` from the GitHub clone, the tree c364d3c8: main a64549cc
merged), CARGO_HOME, CARGO_TARGET_DIR=~/.cache/amphora-perf-target/postgres and
CARGO_INCREMENTAL=0 as RULES rule 6 says, nice -n 10.

| Run | Needs | Command | Result |
|---|---|---|---|
| r332, 05:07Z | cpus=2,mem=8g | `bash scripts/check-production.sh` | exit 0; `engine-ratchet: 10903 left in the engine's production code across 17 lints, as the ratchet holds` |
| r378, 05:18Z | cpus=3,mem=10g, jobs 3 | `cargo fmt --all --check` | exit 0 |
| r378 | same | `cargo test -p amphora-engine --lib -- dialect:: vtab alter --test-threads=3` (`Compiling amphora-engine` in the log) | 35 passed, 0 failed; T-PG.7.3 to 7.10 ok |
| r378 | same | `cargo test -p amphora-pg-catalog` | 2 + 5 passed, 0 failed |
| r378 | same | `cargo clippy -p amphora-engine -p amphora-pg-catalog --all-targets --locked -- -D warnings` | exit 0 (45 s) |

The load average was 51 at the start of r378 and 48 at r390: other lanes' runs.

After the checks main moved only in crates/amphora and crates/api (3a58a13f). `cargo tree -p
amphora-engine` and `-p amphora-pg-catalog`, `-e normal,build,dev`, list amphora-engine-ext, -macros,
-parser and amphora-pg-catalog and nothing else of the workspace: no path to either, so no re-run
(RULES rule 6, rev 00:00Z). `cargo fmt --all --check` was run again on that tree: exit 0.
