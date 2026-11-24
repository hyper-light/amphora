# Lane PG19 (postgres), 2026-11-24: unit 9, TPC-B's statements on the engine

Hour: 2026-11-24T07:18:16Z to 08:18:16Z (from the lane's first command). Branch `perf/postgres-pg19` from main
04aee7c9. Not a speed change: nothing timed, no A/B.

## Losses first

- The statements don't run through the listener, psql or pgbench. They run on the wire core's session (the one
  `crates/amphora/tests/postgres_protocol.rs` drives) over an engine connection made in the test. The listener
  (unit 2) gives its sessions no database yet, so a client still gets `0A000` for every statement there: what
  is left is for the listener to open a database of the PostgreSQL dialect and give each session an `Executor`.
- `aid > 1` was asked (it is not TPC-B's) and answered `0A000`: the parser's ported rules have `=`, `+`, `-` and
  `/` of `a_expr`'s operators. That statement was dropped from the list, and the server's answers were taken
  again from the shorter one.
- `ROLLBACK` is not in the parser's rules (its tree has `BEGIN` and `COMMIT` alone), so the failing block ends
  by `END`, which rolls back, and no test asks for `ROLLBACK`.
- Isolation: an explicit transaction is the engine's own, which reads one snapshot for its whole life. READ
  COMMITTED (a snapshot per statement, section 8) is unit 14's and the waits of unit 14 and 15 aren't built,
  so a second session that updates a row the first has changed waits for nothing: it is `40001`. The two
  sessions of T-PG.9.1 don't write one row at once.
- A function over a column is found by its name on every call (`Func::Dialect`), which section 4 says
  `Func::Pg(ProcOid)` is to replace. Functions that read the session's `TimeZone` or `search_path` over a
  column are refused `0A000`.
- ErrorResponse has severity, SQLSTATE and message, and no position, file, line or routine. The test compares
  the severity and SQLSTATE.
- Built and not compared with a server: several statements in one Query as one implicit transaction, BEGIN twice
  in a block and END outside one (their notices), a NULL in a row, text and int8 columns, and the notices.
  `bpchar`'s padding and the other output functions of T-PG.8.2's values are pg-analysis's own tests.
- Lowering refuses `0A000` for a `Param` (simple protocol has none), a relabel other than text and varchar's, a
  cast by way of text over a column, and a SELECT of no columns. `SET` is refused in analysis.
- `LimboError::Postgres` is a new variant of the engine's error enum. `check-production`'s counts and the engine's
  clippy on Linux are what say no consumer needed a new arm.

## What is built

- `crates/pg-analysis/src/lower.rs`: a typed statement's canonical engine text, its constants computed once
  (`func::eval`) and bound, comparisons over stored keys the engine's own, other functions over a column
  `pg_proc_<OID>(...)`. `Session.now` gives CURRENT_TIMESTAMP its value.
- `crates/engine/dialect/postgres.rs`: the PostgreSQL dialect resolves and runs `pg_proc_<OID>` on
  amphora-pg-analysis's bodies; `to_engine` and `from_engine` are section 5's storage of the unit's types; a
  refusal leaves the engine as `LimboError::Postgres` with its SQLSTATE.
- `crates/amphora/src/postgres/execute.rs`: an `Executor` over an engine connection of that dialect. RowDescription
  and DataRow in text (a bpchar padded to its width), CommandComplete, 19's transaction states (`I`, `T`, `E`),
  `25P02` after a refused statement in a block, several statements in one Query as one implicit transaction.
- `Session::with_statements` gives a session its `Statements`; without one it answers `0A000`, as the listener's
  do.

## Answers from a real 19 server

PostgreSQL 19.0 (Debian 19.0-1.pgdg13+1), the postgres:19 image, in a container of the lane's own on adacachy2,
`POSTGRES_HOST_AUTH_METHOD=trust`, removed when each run ended. `scripts/postgres-tpcb-answers.py` speaks
protocol 3.0 to it and writes every backend message; pgbench's tables are its own `createTables` DDL.

- Statements: 41 (`crates/amphora/tests/tpcb/statements.txt`), two sessions: each account, teller and branch row
  selected, updated and inserted, a whole TPC-B transaction in each session, a second session reading what the
  first has and has not committed, an update past int4's range (`22003`) whose block then refuses `25P02` and
  ends in ROLLBACK, `SELECT 2147483647 + 1` (`22003`), a missing row, and the history rows with their
  CURRENT_TIMESTAMP.
- Compared message by message (T-PG.9.1): RowDescription's names, type OIDs, typlen, typmod, format and attnum
  (whether a table OID is there, not which: servers number tables differently), DataRow's text exactly, tags,
  SQLSTATEs, ReadyForQuery's state. The CURRENT_TIMESTAMP column (`mtime`) is compared by shape.
- Control: see the runs below.

## Runs

All on adacachy2, `sync: false` from the GitHub clone (`~/.cache/amphora-perf/postgres-remote`), CARGO_HOME,
CARGO_TARGET_DIR ~/.cache/amphora-perf-target/postgres, CARGO_INCREMENTAL=0, nice -n 10. The load is at the run's
start. The branch was squashed before the push: the code's tree, 13b176bf's, is what the last runs checked.

| Run | Tree | Command | Result |
|---|---|---|---|
| r1072, 07:49Z, cpus=8 | 4240fffb | `cargo test -p amphora --test postgres_protocol t_pg_9_1` | The red run: FAILED on one statement, `aid > 1` (the server answers two rows, here `0A000`), the other 41 agreeing. `aid > 1` was then dropped. |
| r1081, 07:52Z, cpus=8, load 21.4 31.3 43.1 | b1718a16 | `cargo test -p amphora-pg-analysis`, `-p amphora --test postgres_protocol --test postgres_listener`, `-p amphora-engine --lib -- dialect:: vtab alter` | 2, 3 and 5 passed; 13 and 5 passed; 35 passed, 3,195 filtered out (clippy's run in the same command failed on two lints in lower.rs's tests) |
| r1098, 07:58Z, cpus=2, load 22.6 29.0 38.9 | 2bc49000, main 67cc7e69 merged | clippy failed on the same two lints; `cargo test -p amphora-pg-analysis -p amphora-pg-catalog` and `-p amphora --test postgres_protocol --test postgres_listener` | 2, 3, 5 passed and 2, 5 passed (pg-catalog); 13 and 5 passed |
| same | same | `bash scripts/check-production.sh` | exit 0. The log shows `Compiling amphora-engine`, `Checking amphora-engine-parser`, `Checking amphora-pg-analysis`, `Checking amphora`. `engine-ratchet: 10617 left in the engine's production code across 17 lints, as the ratchet holds` |
| r1120, 08:02Z, cpus=4, load 63.1 49.0 44.9 | eba20acc | `cargo clippy -p amphora-pg-analysis -p amphora-pg-catalog -p amphora-engine -p amphora --all-targets --locked -- -D warnings` | Failed on six lints, all in postgres_protocol.rs's new tests (mended in 13b176bf). pg-analysis, engine and amphora were checked: `Checking amphora-engine`, `Checking amphora-pg-analysis` |
| r1127, 08:02Z, cpus=1 | eba20acc | the control: a container of the lane's own, the 19.0 server, the same list and `scripts/postgres-tpcb-answers.py` | exit 0: 151 lines, one differs from the committed answers, the CURRENT_TIMESTAMP's value |
| r1142, 08:05Z, cpus=2, load 54.9 55.1 48.2 | 13b176bf | the clippy command above | exit 0 |
| same | same | `cargo test -p amphora --test postgres_protocol` | 13 passed, 0 failed |
| this machine | this branch | `cargo fmt --all --check`, `python3 scripts/test_requirements.py` | pass |

Waiting for room: the macOS ask with needs alone stayed queued behind others and was stopped unstarted twice; no macOS
or Windows run was made of this branch. adacachy2 had 0 idle cores of 32 at 07:56 to 07:58Z, so the 8- and 4-core
asks were refused and the runs above ended at 2 cores. `r1137` (clippy and tests, 08:04Z) was lost when kiln's
server answered late and no longer knew it.

The first capture (r1040, 07:41Z, an earlier list whose last block ended by `ROLLBACK`) and the second (07:48Z, the
list as it is but for `aid > 1`) are not checked in as such: the committed answers are the second's with its
last section (that statement's) removed, and r1127's control is the same list and statements run again.
