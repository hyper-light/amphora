# Lane PG24 (postgres), 2026-11-24: `pgbench -i` and COPY FROM STDIN, as a 19.0 server has them

Hour: 2026-11-24T11:38:56Z (the lane's first command) to 12:38:56Z (wind-down from 12:33:56Z). Branch `perf/postgres-pg24` from main
bba0f9fd. Unit 11 of docs/design/postgres.md section 11. Not a speed change: nothing timed, no A/B, every build a debug build.

## Losses first

- **`pgbench`'s built-in scripts don't start.** They ask `select count(*) from pgbench_branches` and the partition query first, unit 12's.
  TPC-B ran here by a custom script with the built-in's ranges over the tables `pgbench -i` left.
- **`pg_index` and `pg_constraint` aren't served** (nor pg_constraint's other relations; no unit has them). The keys were checked by what
  they refuse (the same INSERTs and second ALTERs on both servers), and by the index Amphora builds, never by pg_index's rows.
- `pgbench -i`'s step `f` (foreign keys), and partitions, aren't built: 0A000. The steps that are: d, t, g, G, v, p, six of the seven.
- TRUNCATE is every row deleted as a version: it costs a version a row (100,000 in pgbench_accounts) and isn't 19's lock-and-swap. Other sessions' open
  snapshots go on seeing the rows, where 19's ACCESS EXCLUSIVE lock holds them up.
- VACUUM and VACUUM ANALYZE do nothing but check their tables and the block. No statistics are kept for ANALYZE to write.
- ALTER TABLE ADD PRIMARY KEY: one NOT NULL column. A nullable column is `0A000` where 19 makes it NOT NULL; several columns have no ported rule.
- COPY FROM STDIN is text format only. CSV, binary, DELIMITER, NULL, HEADER, ENCODING, ON_ERROR, WHERE, COPY TO and files refuse `0A000`. A row that meets a key
  another session's open transaction holds is refused `40001`; 19 waits for it.
- Values are the unit's: int4, int8, text, `char(n)`, timestamp, timestamptz. A timestamp that isn't ISO is `0A000` where 19 says `22007` or `22008`.
- generate_series is of integers, in FROM, aliased. A NULL literal has no ported action, so its NULL bounds come as parameters; one that ends within a step of
  the largest bigint refuses `0A000`, as does INSERT of a SELECT from anything but one series, or from a table.
- DETAIL and HINT aren't made (section 4) and neither is compared, so "Key (aid)=(1) is duplicated." and "Failing row contains" are missing.
- A reloption's word value (`vacuum_index_cleanup=auto`) needs `def_arg`'s unported rules: `0A000`. `toast.` ones refuse `0A000`.
- `client_min_messages` and `search_path`: pgbench 19 sends neither in `-i` (log_statement=all on a 19.0 server showed no SET), so there was nothing to take.
- The clients' 32-client part was not rerun. The kiln server gave no answer to several runs of the hour (11:51Z to 12:06Z), asked again: about ten minutes.

Found on the way, left: a SELECT that fails as it runs sent its ErrorResponse without RowDescription in the simple protocol (19 sends it first; fixed here, as
generate_series's step 0 met it); the engine's `generate_series` isn't in the PostgreSQL dialect's schema, so it is the engine's recursive query.

## What is built

COPY FROM STDIN by REL_19_0's src/backend/commands/copy.c, copyfrom.c and copyfromparse.c (the CopyData, CopyDone, CopyFail and Flush and Sync rules of
`CopyGetData`, copyfromparse.c:246; the text reader, :1270 to :1880; the batch limits, copyfrom.c), its CONTEXT (`CopyFromErrorCallback`) and FREEZE's two
refusals; TRUNCATE, VACUUM (`PreventInTransactionBlock`, xact.c:3745), DROP TABLE IF EXISTS and its notices, ALTER TABLE ADD PRIMARY KEY's `42P16`; a heap's 26
storage parameters (reloptions.c); generate_series in FROM; INSERT of a SELECT. Eleven grammar rules joined the ported ones (293 in all): the second and
later items of `reloption_list`, `copy_generic_opt_list` and `columnList`; a bare `reloption_elem`; an empty `opt_analyze` and copy option; `ColLabel` of any
keyword; COPY's column list and FORMAT. docs/design/postgres.md section 4, "What COPY and `pgbench -i` have so far", has the rules. A key or a NOT NULL that
refuses a row is now `23505` or `23502` with 19's text, where it was `23000` with the engine's.

## Against a PostgreSQL 19.0 server

PostgreSQL 19.0 (Debian 19.0-1.pgdg13+1), the postgres:19 image on adacachy2, containers of the lane's own, removed as each run ended.

**Messages (T-PG.11.1).** `scripts/postgres-extended-answers.sh` sends the 174 steps of `crates/amphora/tests/copy/steps.txt` and records the 578 backend messages
they were answered with (answers.txt, md5 5b57a17dc1e6ef8df8e98c7094bd162a: 109 CommandComplete, 174 ReadyForQuery, 70 ErrorResponse with SQLSTATE, message and
CONTEXT, 56 CopyInResponse, 112 DataRow, 32 RowDescription, 5 NoticeResponse, the rest of the extended protocol's). T-PG.11.1 sends the same bytes to the wire
core, whole frames and then a byte at a time, and every message is the same. T-PG.11.2 reads 4,000 data streams whole, a byte at a time and seven at a time.

**pgbench and psql 19** (`scripts/postgres-clients.sh`, its `pgbench -i` part, on the binary of 6feb0fad and again on the one of 536ad624's tree: exit 0, 43 comparisons, each the same): `pgbench -i -s 1 -I dtgvp` prints the
same 11 lines on both (its notices and tuples, times aside) and leaves the same rows, sorted, in all four tables (1 branch, 10 tellers, 100,000 accounts, 0 history;
accounts' 9,488,895 bytes); `-I dtGvp` the same, from its server-side INSERT of a SELECT. After each, six INSERTs that a key or a NOT NULL refuses and the three
second ADD PRIMARY KEYs give the same SQLSTATE and text. TPC-B at `-c 8 -t 100 --random-seed=42` by a script with the built-in's ranges, over those tables,
at `-M simple`, `extended` and `prepared`: 800/800 transactions, 0 failed, on both, and the four tables row for row the same after each (history 801, 1601 and 2401 rows).
The rest of the script (psql's statements, `\bind` and pipelines, the custom tables' TPC-B at 1, 2 and 8 clients) ran before the pgbench -i part on the same binary and was the same in every comparison;
its 32-client part didn't run (the run ended first).

`log_statement=all` on a 19.0 server shows what `pgbench -i` sends: a `drop table if exists`, four `create table`, `begin`, `truncate table`, the relkind query for each
table, a `copy ... with (freeze on)` for each, `commit`, four `vacuum analyze`, three `alter table ... add primary key` (step G: three `insert ... select ... from generate_series`
for the COPYs). No SET is among them: `client_min_messages` and `search_path` are sent by no step.

## Red runs

| Change | What T-PG.11.1 says |
|---|---|
| M0: every COPY refused `0A000`, as before the lane (on 8954e530's tree) | 76 of 174 steps wrong |
| M1: COPY FREEZE not checked | fails before the comparison (a COPY that should refuse goes on waiting) |
| M2: NOT NULL left to the engine (M0 to M2 on 8954e530) | 2 steps wrong |
| M3: TRUNCATE deletes nothing | 10 steps wrong |
| M4: a step of 0 not refused | 2 steps wrong |
| M5: the data's `\.` is a line | 16 steps wrong |
| M6: VACUUM runs in a block (M3 to M6 on main as landed) | 3 steps wrong |

Reds on the way: the first whole capture, T-PG.11.1 failed at 84 of 175 steps (12:07Z, fd9db632: the PostgreSQL dialect had no `generate_series`, several of the list's forms had no ported rule, one step's `text` column hid the rest,
CopyFail's CONTEXT named no line), then at 9 (28c3ea41), then passed (adf6b598). T-PG.11.1 refuses each change above by a comparison with the server's bytes.

## Runs

All on adacachy2 (load 20 to 40), `sync: false`, the GitHub clone (`~/.cache/amphora-perf/postgres-remote`), CARGO_HOME of the lane's, CARGO_TARGET_DIR
~/.cache/amphora-perf-target/postgres, CARGO_INCREMENTAL=0, nice -n 10. Several asks got no answer from the kiln server in time, and were asked again.

| Run | Tree | Command | Result |
|---|---|---|---|
| r2032, 11:40Z, cpus=8, load 31 | bba0f9fd | `cargo test -p amphora --test postgres_protocol --test postgres_listener --test postgres_isolation --no-run`; `cargo build -p amphora --bin amphora` | exit 0: the build folder warm |
| 11:43Z, cpus=2 | (no tree) | `pgbench -i -s 1` and `-I dtGvp` against postgres:19 with log_statement=all | the statements above |
| 12:00Z, cpus=8 | 64c62bcd | `cargo check -p amphora-pg-analysis -p amphora-engine -p amphora --tests` | no error, one warning |
| r2098, 12:03Z; 12:04Z, 12:05Z, cpus=2 | 64c62bcd, 0787de69, fd9db632's lists | the capture | exit 1 three times, the list's (a series of two billion rows; a COPY of no data; a Sync COPY ignores counted as answered), each fixed |
| 12:07Z, cpus=6 | fd9db632 | the capture (919 lines); T-PG.11.1 | capture exit 0; RED: 84 of 175 steps wrong |
| 12:10Z, cpus=6 | 28c3ea41 | the capture; T-PG.11.1 | RED: 9 steps wrong |
| 12:12Z, cpus=6 | adf6b598 | the capture (926 lines, md5 5b57a17d); T-PG.11.1 | passed |
| 12:13Z, cpus=8 | 6feb0fad | fmt; clippy of the five crates; protocol, listener, isolation; pg-analysis, pg-parser, pg-catalog; build | fmt 0; clippy 101 (a method's name); 16, 11 and 3 passed; pg-analysis RED (a refusal's test on a form the grammar lacks); build 0 |
| 12:14Z, cpus=6 | cd0fe052 | fmt; clippy; the pg crates; check-production | clippy 101 (`drop` of a borrow); pg-parser RED (its count of ported rules, 282 to 293); production 101 (the same) |
| 12:16Z, cpus=6 | 8954e530 | the same | clippy 0; the pg crates passed; check-production 0 (10579) |
| r2125, 12:14Z | 6feb0fad's binary | `scripts/postgres-clients.sh` | every comparison the same up to `-M prepared -c 8`'s; ended with the run, not by itself |
| 12:18Z, cpus=2 | the same | its pgbench -i part | DIFFERENT: the built-in TPC-B doesn't start here (unit 12); the script changed to a custom one |
| r2147, 12:20Z, cpus=2 | the same | its pgbench -i part | exit 0, 43 comparisons the same |
| r2145, 12:22Z, cpus=6 | 8954e530 | M0 to M6, stopped after M2 for the gate | red, red, red |
| r2153, 12:22Z to 12:25Z, cpus=3 | 3b51c84f, main 430e5929 merged | fmt; protocol, listener, isolation; pg-analysis, pg-parser, pg-catalog; clippy; check-production; build | fmt 0; 16, 11 and 3 passed (`Compiling amphora-engine`); passed; clippy 0; 0 (10567); build 0. Landed as 536ad624, main not having moved, its tree the same |
| r2163, 12:25Z, cpus=2, load 32 at its end | 536ad624's tree (the binary of r2153) | its pgbench -i part again | exit 0, 43 comparisons the same |
| r2164, 12:25Z, cpus=4 | main 8d70429d (536ad624 and a record) | M3 to M6 | red, each |
