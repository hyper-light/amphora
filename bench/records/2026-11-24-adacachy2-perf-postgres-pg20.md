# Lane PG20 (postgres), 2026-11-24: psql and pgbench on the listener

Hour: 2026-11-24T08:09:23Z to 09:09:23Z (from the lane's first command). Branch `perf/postgres-pg20` from main
b4ad7122. Not a speed change: nothing timed, no A/B. Every binary here is a debug build, and the tps pgbench prints
is not a result.

## Losses first

- `pgbench -i` and pgbench's built-in scripts don't run. Still refused `0A000`: DROP TABLE, TRUNCATE, COPY, VACUUM,
  `INSERT ... SELECT generate_series` (unit 11), and what pgbench asks at start when it runs a built-in script,
  `select count(*)` and the partition query (unit 12). pgbench ran with `-n -f` on the TPC-B script's own statements;
  its `\set` lines give each client rows of its own (below), and the rows were loaded by psql.
- Two sessions that write one row: Amphora refuses the second at once (`40001`), where 19 waits (units 14 and 15).
  Not run here: pgbench's clients write rows of their own.
- A transaction keeps one snapshot, and is refused `40001` at COMMIT when a row it read was written by a commit since
  (the storage's Precision validation, section 8). 19 at READ COMMITTED takes a snapshot for each statement and
  commits. Unit 14. The first lists had a cross-session read in an open transaction (r1279, r1301); it was moved out of
  that transaction in T-PG.9.3's list, and it was not run again alone, so this is from section 8 and the code.
- The same validation holds for what a statement reads of the catalog: a transaction open while another session's
  CREATE TABLE commits is refused at its COMMIT. From the code, not run.
- An error's DETAIL and HINT aren't made. 19 gives them for two statements of the list (42703's HINT, 42883's DETAIL
  and HINT, seen in r1279). T-PG.9.3 compares severity, SQLSTATE, message and position, and
  `scripts/postgres-clients.sh` runs psql with `VERBOSITY terse`.
- A SELECT refused part-way sends none of the rows before the refusal, which 19 sends. Not run.
- Statements run on the listener's one thread, each to its end, its answer held whole and not charged to the budget
  (section 2's "Answers stream" isn't built), and an engine connection's own memory isn't charged to the session. A
  transaction that changes the schema holds the pager's header lock until it ends, and another session's schema
  change is refused meanwhile. Not measured.
- The database named at start-up isn't checked: any name opens the listener's one database, where 19 refuses `3D000`.
  Start-up settings other than the encoding and the application name aren't applied, as before.
- CREATE TABLE takes columns of int4, int8, text, char(n), timestamp and timestamptz with NOT NULL, and `fillfactor`;
  ADD PRIMARY KEY takes one NOT NULL column. A second primary key is refused with the engine's message, `XX000`,
  where 19 says `42P16`.
- T-PG.9.2 was taken (lane PG19's lowering tests), so this unit's test is T-PG.9.3.
- T-PG.2.1's statement changed from `SELECT 1` to `SELECT 1 UNION SELECT 2`, since `SELECT 1` is answered now
  (section 4, unit 9's "the listener still runs none"); its assertion (`0A000`) is the same. T-PG.2.2's live session
  answers `T` where it answered `E`.
- No macOS or Windows run: the macOS run asked for at the lane's start (r1178) was still queued and was stopped
  unstarted.

## What is built

- The listener's sessions run: `amphora postgres --listen <addr> --roles <file> --database <file>` opens one database
  of the PostgreSQL dialect on the engine's versioned storage (`execute::Database`), and a session gets a connection
  and an Executor of its own once its role is authenticated. A row's writer takes no writer's lock there, so an open
  transaction holds none (section 2). A session that ends in an open transaction rolls it back.
- Rowids. Found by T-PG.9.3's first run: two open transactions that insert into `pgbench_history` conflicted, because
  the engine gives each the largest rowid its snapshot shows and one. PostgreSQL's rows have no rowid, so the
  PostgreSQL dialect numbers them (`Dialect::next_rowid`); the SQLite dialect's rule is unchanged.
- ROLLBACK and ABORT, `<`, `>`, `<=`, `>=` and `<>` (gram.y 11337, 11366, 15266 to 15276), each by the hash of the C it
  was ported from. Syntax errors name their token, refusals carry their position.
- Transaction blocks are xact.c's states, so several statements in one Query take BEGIN, COMMIT and ROLLBACK. pgbench's
  script at `-M simple` sends each statement in a Query of its own; psql's `-c` sends several in one.
- CREATE TABLE and ALTER TABLE ADD PRIMARY KEY, as pgbench's createTables and initCreatePKeys send them.

## Against a PostgreSQL 19.0 server

PostgreSQL 19.0 (Debian 19.0-1.pgdg13+1), the postgres:19 image on adacachy2, containers of the lane's own, removed
when each run ended. The clients are that image's psql 19.0 and pgbench 19.0.

- T-PG.9.3: 102 statements (76 on `s1`, 26 on `s2`, one connection each), 287 messages compared: 36 RowDescription, 31
  DataRow, 90 CommandComplete, 22 ErrorResponse, 5 NoticeResponse, 1 EmptyQueryResponse, 102 ReadyForQuery. Byte for
  byte but for a table's OID, a CURRENT_TIMESTAMP's value (by its shape), and of an error or notice the fields named
  above. The list has pgbench's four tables and their keys, its rows, the TPC-B script on both sessions with their
  statements interleaved, ROLLBACK, the comparisons, refusals (42P07, 42701, 42704, 42P01, 42703, 22003, 25P02, 22P02,
  42883, 42601, 22012), warnings (25P01, 25001), and several statements in one Query.
- `scripts/postgres-clients.sh` (both servers side by side, r1318 and, on the landed code, r1343):

| What | Amphora and 19 |
|---|---|
| `psql -a -f setup.sql`: pgbench's tables, 222 rows, three keys | stdout the same, 462 lines, 20,028 bytes; stderr empty on both |
| `psql -a -f statements.sql`: T-PG.9.3's statements in one session, `\echo :SQLSTATE :ROW_COUNT` after each | stdout the same, 774 lines, 9,957 bytes; stderr the same, 20 lines, 1,642 bytes |
| `psql -c` of four statements, BEGIN to END | the same, 8 lines, 48 bytes |
| `pgbench -n -f tpcb.sql -M simple -c 1 -t 100 --random-seed=42` | 100/100 processed, 0 failed on both; the four tables the same row for row |
| the same at `-c 2` (200 transactions, rows of each client's own) | 200/200 processed, 0 failed on both; the four tables the same row for row |

pgbench's table rows are compared sorted (no statement orders them), without `pgbench_history.mtime`.

## Runs

All on adacachy2, `sync: false` from the GitHub clone (`~/.cache/amphora-perf/postgres-remote`), CARGO_HOME of the
lane's, CARGO_TARGET_DIR ~/.cache/amphora-perf-target/postgres, CARGO_INCREMENTAL=0, nice -n 10. The branch was squashed
after r1318; the trees' differences since are named.

| Run | Tree | Command | Result |
|---|---|---|---|
| r1202, 08:11Z, cpus=2, load 67.8 58.7 51.6 | b4ad7122 | `cargo test -p amphora --test postgres_protocol --test postgres_listener --no-run` | exit 0: the build folder warm. |
| r1228, 08:16Z, cpus=2, load 69.7 68.1 57.8 | cae00a15 | listener and protocol tests, pg-parser's suites | Not counted: another lane read pending output of this session's runs by mistake, this one's possibly. What was seen ended exit 0. r1335 ran the same suites again. |
| r1270, 08:21Z, cpus=1 | e7fec938 | the first capture, and the control: `statements.txt` asked of 19 again | `select version()`: PostgreSQL 19.0. The control differs from the committed answers in lines whose first five are RowDescriptions' table OIDs; the rest weren't printed. |
| r1279, 08:22Z, cpus=1 | 2815a98f | capture, then T-PG.9.3 | The red run. 10 statements differ: ALTER TABLE's two refusals (a position 19 doesn't send, another message), s2's INSERT into `pgbench_history` (`40001`, 19 answers INSERT 0 1) and the five after it that turn on it, and two DETAIL or HINT fields. |
| r1301, 08:27Z, cpus=4, load 49.6 58.3 58.6 | 5f7ae14a | T-PG.9.3 with rowids numbered apart, the seek to the last row still kept as a read | Red: 4 statements, s2's END (`40001`) and the three reads that turn on it. |
| r1308, 08:29Z, cpus=4 | f1adf21a | the same without the cross-session read in s2's transaction; `scripts/postgres-clients.sh` | Red on the same END, so that read wasn't its cause. pgbench -c 1: 100/100. pgbench -c 2: 125 of 200 processed, 75 failed (19: 200). |
| r1318, 08:33Z, cpus=4, load 43.4 50.2 55.0 | 6e3db86b, the rowid read kept by no transaction | T-PG.9.3 and the listener and protocol suites; `scripts/postgres-clients.sh` | 7 and 13 passed. Clients exit 0, the table above. pg-analysis's own new test failed on a statement the parser refuses (a key of two columns), dropped from it. |
| r1328, 08:37Z, cpus=3 but CARGO_BUILD_JOBS=4 (a mistake), load 46.6 49.5 53.9 | 9773298e, main fcbdf0c2 merged | clippy, the command below | Failed on two `then_some` in utility.rs's tests. |
| r1331, 08:38Z, cpus=3, load 37.1 46.1 52.3 | 769d670b | clippy; listener and protocol; pg-parser, pg-analysis, pg-catalog | Clippy failed on the mend (`unnecessary_lazy_evaluations`). Tests passed. |
| r1335, 08:41Z, cpus=4, load 23.2 36.7 47.8 | 097095ae, main 6077d42a merged | `cargo clippy -p amphora-pg-parser -p amphora-pg-analysis -p amphora-pg-catalog -p amphora-engine -p amphora --all-targets --locked -- -D warnings` | exit 0 (`Checking amphora-engine`, `Checking amphora-pg-analysis`, `Checking amphora`). |
| same | same | `bash scripts/check-production.sh` | exit 0. `Compiling amphora-engine` in its log. `engine-ratchet: 10617 left in the engine's production code across 17 lints, as the ratchet holds`. |
| same | same | `cargo test -p amphora --test postgres_protocol --test postgres_listener` | 7 and 13 passed. |
| same | same | `cargo test -p amphora-pg-parser -p amphora-pg-analysis -p amphora-pg-catalog` | all passed (4, 3 and 5; 2 and 5; 16, 1, 1, 1, 1 and 1; 2 ignored, the oracle's on-demand tests). |
| r1343, 08:43Z, cpus=4, load 42.1 36.5 45.9 | 0e132eb5 (the cursor left as an append leaves it) | the clippy command | exit 0, every crate above checked. |
| same | same | check-production | exit 0, `Compiling amphora-engine`, 10617 as the ratchet holds. |
| same | same | listener and protocol | 7 and 13 passed. |
| same | same | `cargo test -p amphora-engine --lib -- dialect:: vtab alter rowid autoinc` | exit 0: 90 passed, 1 ignored, 3,167 filtered out (and four tests of one that those run in a process of their own), `Compiling amphora-engine` in its log, t_pg_9_3_transactions_that_insert_beside_each_other_both_commit among them. The versioned storage's own suite ran after the landing, r1358. |
| same | same | `scripts/postgres-clients.sh` | exit 0, every comparison the same: the table above, again. |
| r1358, 08:50Z, cpus=4, load 28.0 35.9 43.2 | ede916ac, main as landed | `cargo test -p amphora-engine --lib -- storage::versioned::sql storage::versioned::cursor storage::versioned::switch` | exit 0: 21 passed, 3,237 filtered out, `Compiling amphora-engine` in its log. The rest of the engine's suites were not run. |
| this machine | the branch | `cargo fmt --all --check`, `python3 scripts/test_requirements.py` | pass |

The landed tree is 0e132eb5's with one test comment moved (83360d86, no code) and main merged since: main moved only in
bench/ and its records, so no run was repeated.

adacachy2 had 0 to 9 idle cores all hour: the 8- and 4-core asks at 08:09Z and 08:10Z and the first at 08:22Z were
refused and asked again smaller.
