# Lane PG23 (postgres), 2026-11-24: the extended query protocol, and the binary formats of the types there are

Hour: 2026-11-24T10:44:30Z (the lane's first command) to 11:44:30Z (wind-down from 11:39Z). Branch `perf/postgres-pg23`
from main 0c0a3b6c. Unit 10 of docs/design/postgres.md section 11. Not a speed change: nothing timed, no A/B, every build a debug
build.

## Losses first

- pgbench's step `g` itself wasn't run (`pgbench -i` is unit 11's): its relkind query was, through T-PG.10.1 and through libpq as psql's
  `\bind` sends it (PQsendQueryParams), not by PQexecParams.
- numeric has no binary format here: Amphora has no values of it (nor of int2, the floats, varchar, date or the arrays), so it refuses `0A000`
  in both formats, at Bind or where the statement's analysis meets it. 19 has one.
- Nothing of a plan is kept between Binds: each analyzes its statement and prepares the engine's again. Not measured.
- A SELECT's rows are held whole from its Bind, charged to the session's budget, until its portal ends: unit 9b's snapshot cursor isn't built. A
  portal bound and never executed has run. An INSERT's or UPDATE's runs at its Execute, as 19's does.
- 19's plan cache moves the refusal of an immutable expression from Bind to Execute once a statement's sixth Bind may take the generic plan
  (`choose_custom_plan`): here it is always the Bind's. No step binds one six times.
- An Execute's answer is sent when the input runs out, before the Sync or Flush 19 may hold it to. Its order and content are the same.
- A Query among messages no Sync has ended commits them; 19 leaves an implicit block open and reports `T` (xact.c's `TBLOCK_IMPLICIT_INPROGRESS`
  after a single-statement Query). libpq refuses to send one (fe-exec.c). Not compared.
- An ErrorResponse's CONTEXT is a Bind parameter's only, its DETAIL and HINT still aren't made (section 4), and neither of those two is compared.
- The comparison leaves out a table's OID (but whether there is one), and a refusal's source file, line and routine.
- FunctionCall is refused as before this lane.
- No macOS run counted: the one asked for waited for room from 11:07Z and was stopped once its tree was stale.

Found on the way, left: the scanner's `unterminated quoted string` lacks 19's ` at or near "'"`; `IS NULL`, TRUE and NULL literals and
`timestamp(0)` have no ported grammar action; a timestamptz's text that isn't ISO is `0A000` where 19 says `22007` or `22008`;
`SELECT $268435455` sizes a list of that many parameters (1 GiB) before it is refused, as 19 does, and here nothing is charged for it.

## What is built

Parse, Bind, Describe, Execute, Close, Sync and Flush as REL_19_0's `src/backend/tcop/postgres.c` (`exec_parse_message` :1406, `exec_bind_message` :1640,
`exec_execute_message` :2126, `exec_describe_*_message` :2645, :2737, `start_xact_command` :2788, the main loop :4839-5090), `pquery.c` (`PortalStart`,
`PortalRunSelect`, `PortalSetResultFormat`) and `portalmem.c` (`AtAbort_Portals`, `AtCleanup_Portals`) do it: statements and portals by name and unnamed, a
portal's rows some at a time, parameters typed by analysis or given, the messages since the last Sync one transaction, an error's skip to Sync,
a failed block's messages, 25P02, 26000, 34000, 42P03, 42P05, 55000, 08P01, 22P03 and 22023 where 19 gives them. Text and binary
formats of bool, `"char"`, int4, int8, text, `char(n)`, name, oid, regclass, timestamp and timestamptz, by each type's `typreceive` and `typsend`; a type with no
binary function is `42883` as in 19. docs/design/postgres.md section 4, "What the extended protocol has so far", has the rules.

Two corrections of the simple protocol came with it: a SELECT refused partway sends the rows before the refusal (compared: the step
`Q|SELECT 1 / (a - 2) FROM n WHERE a < 4`), and a Query that isn't UTF-8 fails its transaction block, as postgres.c's error path does (no step has one; before,
only its answer was sent).

## Against a PostgreSQL 19.0 server

PostgreSQL 19.0 (Debian 19.0-1.pgdg13+1), the postgres:19 image on adacachy2, containers of the lane's own, removed as each run ended.

**Messages.** `scripts/postgres-extended-answers.sh` sends the 252 steps of `crates/amphora/tests/extended/steps.txt` (the bytes it sent are in answers.txt, md5
306ab6691364011468fac5cb58b7d35b) and records 1,024 backend messages: 112 ParseComplete, 144 BindComplete, 13 CloseComplete, 29 ParameterDescription,
59 RowDescription, 32 NoData, 100 DataRow, 13 PortalSuspended, 155 CommandComplete, 4 EmptyQueryResponse, 103 ErrorResponse, 6 NoticeResponse, 254 ReadyForQuery. T-PG.10.1
sends the same bytes to the wire core, whole frames and then a byte at a time, and every message is the same.

**pgbench and psql 19** (`scripts/postgres-clients.sh`, r2018 on main as landed, 0feda2e0, and r1945 on a06603ae: 37 comparisons, each the same, exit 0): TPC-B at `-M extended` and
`-M prepared`, `-c 1 -t 100` and `-c 8 -t 100` (the eight contend for two branches), `--random-seed=42`: 100/100 and 800/800 transactions, 0 failed, on both, and the four tables row for row
the same after each (history 402, 1202, 1302 and 2102 rows); `-M simple` at 1, 2 and 32 clients as before. psql's `\bind`, `\parse`, `\bind_named`, `\close_prepared` and a
pipeline (`\flushrequest`, `\syncpipeline`, an error in it, an implicit transaction undone by it) print the same 96 and 11 lines, its relkind query and its refusals with their CONTEXT among them.

T-PG.10.2: over sockets, an Execute waits for another session's row with what a Flush asked for already sent. T-PG.10.3: prepared statements are charged to the budget, `53200` past it.
T-PG.10.4: 252 steps' messages cut, grown and changed byte by byte, each answered to its ReadyForQuery.

## Red runs

Each part taken out of the landed code (0feda2e0), one at a time, and `cargo test -p amphora --test postgres_protocol --test postgres_listener -- t_pg_10` run
on it (r2005, 11:28Z; its script `mutate.py` in the lane's folder):

| Change | What failed |
|---|---|
| M0: every extended message refused `0A000`, as before the lane | T-PG.10.1 (216 of 252 steps), T-PG.10.2, T-PG.10.3 |
| M1: each Execute is its own transaction | T-PG.10.1 (8 steps), T-PG.10.2 |
| M2: no skip to Sync | T-PG.10.1 (53) |
| M3: portals outlive their transaction | T-PG.10.1 (22) |
| M4: the row limit is ignored | T-PG.10.1 (11) |
| M5: a stable function's refusal is Bind's | T-PG.10.1 (1) |
| M6: every column is text | T-PG.10.1 (10) |
| M7: the unnamed statement outlives a Query | T-PG.10.1 (2) |
| M8: no implicit block in a pipeline | T-PG.10.1 (1: `SET LOCAL` warns again) |
| M10: nothing is charged | T-PG.10.3 |
| M11: an Execute that waited ends in ReadyForQuery | T-PG.10.2 |
| M12: a message malformed inside ends the connection | T-PG.10.1 (1) |

T-PG.10.4 failed under none of them (M0 and M2 among them leave it green: it asks for an end, not for an answer). Not mutated: the snapshot a SELECT's portal has from its Bind
(two steps hold it), what a failed block's old portals answer, a Flush's own send.

## Runs

All on adacachy2 (load 21 to 50), `sync: false`, the GitHub clone (`~/.cache/amphora-perf/postgres-remote`), CARGO_HOME of the lane's,
CARGO_TARGET_DIR ~/.cache/amphora-perf-target/postgres, CARGO_INCREMENTAL=0, nice -n 10. Runs refused for room aren't listed.

| Run | Tree | Command | Result |
|---|---|---|---|
| r1850, 10:44Z, cpus=4, load 22 | 0c0a3b6c | `cargo test -p amphora --test postgres_protocol --test postgres_listener --test postgres_isolation --no-run` | exit 0: the build folder warm |
| r1892, 10:59Z, cpus=4 | 425a5740 | `cargo check -p amphora-pg-analysis -p amphora-engine -p amphora --tests` | no error |
| a foreground run, 11:03Z | 9d7659fa | the capture | exit 124: the script waited for the answer of a message 19 skips; fixed |
| a foreground run, 11:05Z | 1af964ab | the capture | exit 0: 1,478 lines, md5 86d7c782 (the first list, 248 steps) |
| r1924, 11:07Z, cpus=1, load 50 | 9f641d06 | `cargo test -p amphora --test postgres_protocol` | didn't compile: answers.txt missing from the clone |
| r1931, 11:08Z, cpus=1 | 9f641d06 | the same | RED: T-PG.10.1, 18 steps of 248, at both the list's doing (rows compared in the order of a scan an UPDATE had changed, 13; a grammar rule with no ported action, 3) and two found on the way (above); 13 passed |
| r1934, 11:09Z, cpus=4 | e63332ef | the capture (1,508 lines, md5 8ac0cec9); protocol, listener, isolation | 14, 11 and 3 passed |
| r1940, 11:10Z, cpus=4 | a06603ae, main 837071af merged (lane C17) | clippy of the five crates; `cargo build -p amphora --bin amphora` | clippy 101 (a method's name); build 0 |
| r1945, 11:11Z, cpus=2, load 26 | a06603ae | `scripts/postgres-clients.sh` | exit 0, every comparison the same |
| r1955, 11:15Z, cpus=6 | 0f600621 | the capture (1,528 lines, md5 306ab669, the list landed); clippy; protocol, listener, isolation | clippy 101 (`PgError` at 136 bytes); 15, 11 and 3 passed |
| r1964, 11:17Z, cpus=6 | 5db76636, main edc3b885 merged (lanes C18, B27) | clippy; the three; `cargo test -p amphora-pg-analysis -p amphora-pg-parser -p amphora-pg-catalog` | clippy 101 (a function one test binary doesn't use); 15 and 3 passed, the listener's T-PG.10.3 RED (it asked for a byte more than a Close gave back; the test's); the pg crates' passed |
| r1968, 11:19Z, cpus=6 | 4d6d1ca4 | clippy; the three; `bash scripts/check-production.sh` | clippy 101 (two byte strings in a test); 15, 11 and 3 passed; check-production 0, 10591 |
| r1980, 11:22Z, cpus=6 | dc7bc5b1, main ed71a93e merged | clippy; the three; check-production; `cargo test -p amphora-engine --lib -- dialect:: storage::versioned::version storage::versioned::switch storage::versioned::sql pg_catalog` | clippy 0; 15, 11 and 3 passed; check-production 0, 10591; 74 passed |
| r1991, 11:25Z, cpus=6, load 21 | 29f1fadd, main 7a337181 merged (lane C21's engine change) | fmt; clippy; the three; check-production | fmt 0; clippy 0 (every crate `Checking`); 15, 11 and 3 passed (`Compiling amphora-engine`); check-production 0, 10581. Landed as 0feda2e0, main having moved in a bench record alone (20cffc15) |
| r2005, 11:28Z, cpus=8, load 22 | 0feda2e0, main as landed | the red runs above | each red |
| r2018, 11:33Z, cpus=6, load 40 | 0feda2e0, main as landed | `cargo build -p amphora --bin amphora --locked`; `scripts/postgres-clients.sh` | build 0; exit 0, 37 comparisons the same; the 32 clients at `-M simple`: 1600/1600, 0 failed, each table moved by -160201 here and by 282841 on 19.0 |
| this machine | the branch | `cargo fmt --all --check`, `python3 scripts/test_requirements.py` | pass |
