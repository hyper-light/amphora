# Lane PG21 (postgres), 2026-11-24: READ COMMITTED's statement snapshots and the wait for a row's holder

Hour: 2026-11-24T08:52:26Z to 09:52:26Z (from the lane's first command; wind-down from 09:47Z). Branch
`perf/postgres-pg21` from main 2b713d77. Unit 14 of docs/design/postgres.md section 11. Not a speed change: nothing
timed, no A/B, every build a debug build. Commit of the code and tests: see the landing below.

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

`RED_RUNS_HERE`

## Runs

All on adacachy2, `sync: false`, the GitHub clone (`~/.cache/amphora-perf/postgres-remote`), CARGO_HOME of the
lane's, CARGO_TARGET_DIR ~/.cache/amphora-perf-target/postgres, CARGO_INCREMENTAL=0, nice -n 10, the machine at load 47 to 68.

RUNS_HERE
