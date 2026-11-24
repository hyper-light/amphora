# Lane PG21 (postgres), 2026-11-24: READ COMMITTED's statement snapshots and the wait for a row's holder

Hour: 2026-11-24T08:52:26Z to 09:52:26Z (from the lane's first command; wind-down from 09:47Z). Branch
`perf/postgres-pg21` from main 2b713d77. Unit 14 of docs/design/postgres.md section 11. Not a speed change: nothing
timed, no A/B, every build a debug build. The landed tree is 81727306's with main's bench and records since (6a29aebf), so no run was repeated.

## Losses first

- The statement that waited is run again whole, where 19 continues it and rechecks the one row (EvalPlanQual, section
  11's unit 15). So: a statement of several rows undoes what it changed before the wait, which another session may change
  in the interval; and a WHERE is judged by the newer snapshot on every row it reads, not on the waited-for row alone. For
  one row reached by its key, and for the 15 interleavings below, the answer is 19's. 19's documentation example (an
  UPDATE that adds one to a counter while another session deletes by the old value) is such a difference; it was not
  run, since DELETE isn't ported.
- A deadlock is refused `40P01` at the wait that closes the ring of waits. 19 waits out `deadlock_timeout` and refuses the
  session whose check runs first, so the victim can differ. Unit 15. The interleavings compared with 19 have none.
- `lock_timeout` isn't there (unit 15): a statement waits until its holder ends, and a client can't cancel the wait
  (CancelRequest is a later unit; 19 also doesn't notice a client that goes away, unless
  `client_connection_check_interval` is set).
- SERIALIZABLE is the storage's Precision validation with the wait; 19's SSI isn't ported (section 8), so the level is
  named and waits as 19's, and may refuse `40001` where 19's doesn't.
- Not chosen by the level: READ ONLY, READ WRITE, DEFERRABLE, SESSION CHARACTERISTICS (refused `0A000`, as before);
  SAVEPOINT too. `BEGIN ISOLATION LEVEL ...` inside an open block is refused `0A000`, where 19 warns and may refuse
  `25001`.
- A refusal in a REPEATABLE READ or SERIALIZABLE transaction for a row another session DELETEd reads "concurrent update",
  where 19 says "concurrent delete" (no DELETE is ported; not run).
- A transaction open while another session's schema change commits is refused at COMMIT, as on main before (PG20).
- The brief named two specs of 19's isolation suite: `update-conflict-out` is a SERIALIZABLE spec of three sessions (fetched
  from REL_19_0 and read: its setup is `BEGIN TRANSACTION ISOLATION LEVEL SERIALIZABLE`), so it is SSI's unit, not this one,
  and `lost-update.spec` does not exist in REL_19_0 (HTTP 404). Neither was run. 19's `src/test/isolation/specs` was not
  searched further for READ COMMITTED specs within the statement subset.
- Not measured: what the wait, the renewed snapshot (one more lock at each statement of a block), or the atomic read at
  each transaction's end cost.
- The only DELETE and SELECT ... FOR UPDATE waits, unique-key waits (INSERT against INSERT) and multi-session queues of
  more than one waiter are from the code, not compared with 19.
- No macOS run of these tests counted toward the landing (RULES rule 6: check-production counts on Linux).

## What is built

- The level is the session's (`Level`, `crates/engine/storage/versioned/switch.rs`): BEGIN and START TRANSACTION ISOLATION
  LEVEL, SET TRANSACTION ISOLATION LEVEL, and SHOW transaction_isolation, READ COMMITTED the default. The grammar actions
  they need are ported by the hash of gram.y's C text (rules 218, 244 to 247, 1490, 1506, 1511 to 1513).
- Each transaction of a session has its own rules (`Rules`, `version.rs`): READ COMMITTED and REPEATABLE READ are validated
  as `Snapshot`, SERIALIZABLE by the storage's own; and all three wait for a row's holder.
- READ COMMITTED's snapshot moves to every commit published at each statement of a block (`Clock::renew`).
- A statement that changes a row another open transaction changed stops with `LimboError::RowTaken(holder)`, its changes
  undone to a savepoint put before it, its transaction kept; `RowTaken(None)`, a change committed since the snapshot,
  restarts a READ COMMITTED statement on a new snapshot and refuses the others with `40001`.
- The session then waits for the holder's end by a `Waker` (`Clock::wait_for`, woken in `Clock::forget`); no timer. The
  listener's other sessions run meanwhile (the wait is a future of the session's task), and the Query goes on from the
  statement that stopped. A wait that would close a ring is refused `40P01`.
- A transaction end takes a lock for the waiters only when some wait (`Clock::waiting`, an atomic read otherwise).

## Against a PostgreSQL 19.0 server

PostgreSQL 19.0 (Debian 19.0-1.pgdg13+1), the postgres:19 image on adacachy2, a container of the lane's own, removed as
each run ended; its answers are taken by `scripts/postgres-isolation-answers.sh` (the wire protocol, one step at a time;
a step that waits is known to wait by `pg_stat_activity`'s lock wait before the next step).

T-PG.14.1 (`crates/amphora/tests/postgres_isolation.rs`): each step run by a session's `Executor` on one database of
Amphora's, two sessions. 15 interleavings, 149 steps, 9 of them waits, 33 RowDescription, 33 DataRow, 138 CommandComplete,
4 ErrorResponse, 1 NoticeResponse and 140 ReadyForQuery (the transaction state each ends in) compared with 19's answers
(severity, SQLSTATE, message and position of a refusal; a table's OID by whether there is one). All the same:

| Interleaving | What 19 answers, and Amphora |
|---|---|
| update-waits-for-commit | s2's UPDATE waits for s1's, then is applied to the value s1 committed (111) |
| update-waits-for-rollback | after s1's ROLLBACK, s2's UPDATE is applied to the value before (110) |
| update-recheck-skips-row-no-longer-matching | the WHERE is judged on the committed value: UPDATE 0 |
| update-recheck-still-matches | it still matches: UPDATE 1, value 6 |
| update-new-value-matches-only-after-commit | the snapshot doesn't see it: UPDATE 0 at once, no wait |
| read-committed-statement-snapshot | the same transaction's second SELECT sees a commit made between them |
| read-committed-lost-update | both read 10, the second writer waits and overwrites: 12 |
| repeatable-read-refuses-changed-row | the same SELECT twice, then `40001`, then `25P02` |
| repeatable-read-waits-then-refuses | waits for the holder's commit, then `40001` |
| repeatable-read-waits-then-holder-rolls-back | waits, then is applied: 15 |
| repeatable-read-snapshot-is-its-first-statements | a commit after BEGIN and before the first statement is seen |
| rows-of-their-own-do-not-wait | different rows, no wait |
| a-waiter-in-a-block-keeps-its-earlier-writes | the statement alone is undone, the block's earlier write kept |
| a-query-of-several-statements-resumes | a Query of three statements, the second of which waits |
| levels-and-set-transaction | SHOW names the levels, READ UNCOMMITTED its own name; SET TRANSACTION before and after a query (`25001`) and outside a block (`25P01`) |

T-PG.14.2 (the wait graph's refusal) and T-PG.14.3 (over sockets, a session that waits is answered by its holder's commit
and no other session is stopped), the same test files.

## Red runs

The landed code's tests were red before a fix each time in this hour:

- T-PG.14.1's first run (07382265, above): the list compared two rows without an order, where 19 lists a row after its update;
  the list now reads one row at a time. Reading 19's answers found READ UNCOMMITTED's own name in SHOW.
- The clippy runs r1522 and r1534 (above) and the fixes to them, no behaviour.
- Found by reading the validation's own doc while the runs went (no run showed it): READ COMMITTED and REPEATABLE READ
  are validated as `Snapshot`, which doesn't refuse an index built beside a commit that adds rows, so a transaction that
  changes the catalog is now validated in full (`Txn::validate_fully`); the test
  `t_pg_14_1_an_index_built_beside_a_commit_is_refused_at_any_level` is its check. Not shown red on the code before the fix
  in this hour (no run of that test on it).


## Runs

All on adacachy2, `sync: false`, the GitHub clone (`~/.cache/amphora-perf/postgres-remote`), CARGO_HOME of the
lane's, CARGO_TARGET_DIR ~/.cache/amphora-perf-target/postgres, CARGO_INCREMENTAL=0, nice -n 10, the machine at load 47 to 68.

| Run | Tree | Command | Result |
|---|---|---|---|
| r1367, 08:53Z, cpus=4, load 24.0 33.3 41.1 | 2b713d77 | `cargo test -p amphora --test postgres_protocol --test postgres_listener --no-run` | exit 0: the build folder warm. |
| r1425, 09:06Z | 1627674c | `cargo test -p amphora-engine -p amphora -p amphora-pg-parser -p amphora-pg-analysis --no-run` | exit 0 (an earlier run of the same, r1405, not counted: its output was cut by a pipe). |
| a foreground run, 09:08Z | 07382265 | the capture, 412 lines (md5 9c2db621), 12 interleavings | `select version()`: PostgreSQL 19.0. |
| a foreground run, 09:10Z, cpus=4 | 07382265 | `cargo test -p amphora --test postgres_isolation` | RED. T-PG.14.2 passed; T-PG.14.1 stopped at `select id, v from k where id <= 2`, 19's rows in another order than Amphora's. No ORDER BY is ported, so the list asks for one row at a time instead; READ UNCOMMITTED's name was found by reading the answers (19 says `read uncommitted`, Amphora said `read committed`). |
| r1463, 09:12Z, cpus=2, load 67.8 55.5 47.0 | 7473cd97 | the capture again (420 lines, md5 47467ebe); `cargo test -p amphora --test postgres_isolation --test postgres_listener` | 2 and 8 passed. |
| r1487, 09:15Z, cpus=3, load 59.8 57.0 49.0 | 47ee6d47 | the capture again (498 lines, md5 af7926eb; three interleavings more); isolation | 2 passed. |
| r1501, 09:17Z, cpus=4, load 58.9 59.0 50.7 | aacc4354 | `cargo test -p amphora-engine --lib -- storage::versioned::version storage::versioned::switch storage::versioned::sql`, then isolation, listener, protocol | 43 passed (3,217 filtered out, `Compiling amphora-engine`), t_pg_14_1_a_waiting_writer_is_told_once_by_the_holders_end and t_pg_14_2_a_wait_that_closes_a_ring_is_refused among them; 2, 8 and 13 passed. |
| r1522, 09:20Z, cpus=3 | 3e0cc51b, main 77f1e18f merged | clippy | Failed: one collapsible `if` in execute.rs. Stopped. |
| r1534, 09:22Z, cpus=2, load 51.8 56.6 52.1 | 6163aa17 | clippy; check-production | clippy failed on two unused items and two string slices in the new test; check-production exit 0, the ratchet at 10617. |
| r1548, 09:25Z, cpus=3, load 49.0 53.7 51.9 | 98d1b07e | `cargo clippy -p amphora-pg-parser -p amphora-pg-analysis -p amphora-pg-catalog -p amphora-engine -p amphora --all-targets --locked -- -D warnings` | exit 0 (`Checking amphora-engine`, `Checking amphora`). |
| same | same | isolation, listener, protocol; `cargo test -p amphora-pg-parser -p amphora-pg-analysis -p amphora-pg-catalog` | 3, 8 and 13 passed; the pg crates' suites all passed (4, 3, 5; 2, 5; 16, 1, 1, 1, 1 and 1; 2 ignored, the oracle's on-demand tests). |
| same | same | `bash scripts/check-production.sh` | exit 0. `engine-ratchet: 10617 left in the engine's production code across 17 lints, as the ratchet holds`. |
| r1564, 09:28Z, cpus=4, load 40.4 48.9 50.5 | b734a56d, main 925c73fb merged | fmt, `scripts/test_requirements.py`, the clippy command; isolation, listener, protocol; `cargo test -p amphora-engine --lib -- storage::versioned dialect::` | clippy exit 0; 3, 8 and 13 passed; the engine 204 passed, 1 ignored, 3,056 filtered out, in 211 s; check-production exit 0, 10617 as the ratchet holds. |
| r1585, 09:35Z, cpus=4, load 14.7 29.3 41.4 | 81727306, main af8ca01c merged (lane C9's and V22's engine changes) | fmt, clippy; isolation, listener, protocol; `cargo test -p amphora-engine --lib -- storage::versioned::version storage::versioned::switch dialect::` | clippy exit 0 (every crate above checked); 3, 8 and 13 passed; 56 passed, 3,208 filtered out; check-production exit 0, 10616 as the ratchet holds. |
| this machine | the branch | `cargo fmt --all --check`, `python3 scripts/test_requirements.py` | pass |

