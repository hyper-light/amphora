# Lane PG25 (postgres), 2026-11-24: pgbench's built-in scripts start and run, as on a 19.0 server

Hour: 2026-11-24T12:30:36Z (the lane's first command) to 13:30:36Z (wind-down from 13:25:36Z). Branch `perf/postgres-pg25` from main
a7d4af72. Unit 12 of docs/design/postgres.md section 11. Landed on main as 920eb42a (13:15:45Z). Not a speed change: nothing timed, no
A/B, every build a debug build.

## Not checked: kiln's runs stopped starting on adacachy2 at 13:01Z

Runs begun before it finished (the gate, 13:03:23Z; the pgbench comparison, 13:04:02Z). Three asked for after it (r2276, r2296, r2300) were
accepted, gave no output, and were stopped. These remain, each with its command:

- **No red run without the change.** Asked for and not had: `T-PG.12.1` on main before the lane, and the ten mutations of
  `~/.cache/amphora-perf/pg25/mutants.py` on adacachy (`python3 mutants.py` in the adacachy2 clone, about 40 s each). What there is: reds
  of `T-PG.12.1` on the lane's own unfinished trees (58 of 221 steps, then 1), below.
- **pgbench's own run was on an older binary.** `PARTS=builtin scripts/postgres-clients.sh` ran on bec17c67's binary, from before main's
  foreign keys (C26), compounds (C27) and log (B29) were merged, and before the last change to `lower.rs` (a result's origin through a subquery; the
  numbers of the bound parameters). Of the landed tree, T-PG.12.1 and the suites have run. The gate's binary is kept:
  `PARTS=builtin bash scripts/postgres-clients.sh ~/.cache/amphora-perf-bin/postgres/e0c9236d ~/.cache/pg25-clients` (9 minutes on adacachy2).
- The rest of that script (psql's statements, `\bind`, pipelines, the custom scripts, `-c 32`, `pgbench -i`'s step G) wasn't rerun on any of the lane's binaries.
- **Two tests written and never compiled**, on `perf/postgres-pg25` alone (0f8a9020), not on main: a binary array cut anywhere or with a byte of it
  another, and 20,000 hostile array texts. `cargo test -p amphora-pg-analysis --lib`, clippy of the crate, then main.
- main moved after the gate in records and `crates/amphora/tests/crash.rs` (B29's); the landed tree is the gated one, e0c9236d, plus those.

## Losses

- **Step `f` of `pgbench -i` isn't built**: `0A000`. It sends five `alter table ... add constraint <name> foreign key (<column>) references <table>`
  (REL_19_0 src/bin/pgbench/pgbench.c:5283). It needs: the grammar's ConstraintElem FOREIGN KEY, `opt_column_and_period_list`, `key_match`, `key_actions`
  and named table constraints (none is ported); analysis of the referenced table's primary key and the types' operator
  (`transformFkeyGetPrimaryKey`, `ATAddForeignKeyConstraint`); the existing rows checked at ALTER (`RI_Initial_Check`); each INSERT and UPDATE of
  a referencing row refused `23503` with 19's text, each DELETE or key UPDATE of a referenced one too; and the referenced row kept as it is until the
  transaction ends (`FOR KEY SHARE`, src/backend/utils/adt/ri_triggers.c's `RI_FKey_check`), for which a search of the engine and the listener found no lock. Read from the
  sources, nothing of it built or run. It didn't fit the hour.
- **A target's `AS name` isn't ported by the grammar** (`target_el: a_expr AS ColLabel`), so `select count(*) as n` is `0A000`; nor are FALSE, NULL, OR, NOT,
  IS NULL, `*` and `%`, an ORDER BY without a LIMIT, of several items, or without ASC, several FROM items, a parenthesized join, RIGHT, FULL, NATURAL and USING
  joins, a subquery in FROM that isn't LATERAL, HAVING and DISTINCT. `pgbench`'s own statements use none of these.
- `count` is the only aggregate. A LATERAL subquery is a SELECT of expressions alone (no FROM, WHERE, GROUP BY or aggregate), and not under an outer join. A join on
  the right of a join, several generate_series and a whole-row reference are `0A000`.
- GROUP BY and ORDER BY take the unit's types (bool, `"char"`, name, int2, int4, int8, oid, regclass, text, `char(n)`, timestamps): the rest `0A000`.
- Arrays: no column of one, no `ARRAY[...]`, no subscript or operator, no cast from one element type to another
  (`array_position('{1,2}'::int4[], 2::int8)` is `0A000`, where I expect 19 to find it: not compared), and `char(n)`'s aren't built.
- `pg_class` and `pg_attribute` have no rows of `pg_namespace`, `pg_partitioned_table` and `pg_inherits` (nor of any catalog but the four bootstrap writes itself),
  so `select relkind from pg_class where oid = 'pg_inherits'::regclass` gives no row, where a 19 server's pg_class has one (not compared). Their names resolve.
- A constant a subquery gives is computed as the statement runs, not as it is planned, so `1 / n` of `lateral (select 0) as o(n)` is refused after its
  RowDescription, where I expect 19 to refuse it before (its planner folds it): not compared.
- The script's pgbench ran 100 transactions a client at scale 1, with a seed: milestone one's check at one client and at eight, not a long run.
- DETAIL and HINT aren't made or compared (section 4).

Lost time: ten minutes (13:03Z to 13:13Z) went to one `run` left at its default wait of 600 s on a run that never started; the red runs would have fitted there. A
capture at 12:58Z ended with no exit code and no output, and was asked again.

## What is built

`GetTableInfo`'s two queries (pgbench.c:5411) and what they need, each rule cited where it is ported: a range table and a join tree
(`transformFromClauseItem`, parse_clause.c:1056; names by parse_relation.c:130, 360, 438, 716, 926, 3723, 3794), GROUP BY, ORDER BY and LIMIT
(parse_clause.c:1883, 2008, 2174, 2369, 2734, 3402, 3546), aggregates' levels, places, nesting and grouping (parse_agg.c:113, 308, 1139, 1350), a
functional dependency on a primary key (19's catalogs' keys lifted from their headers' DECLARE_UNIQUE_INDEX_PKEY), a LATERAL SELECT of expressions put in its
columns' place, `count(*)` and `count(any)`, arrays (arrayfuncs.c:181, 406, 523, 583, 800, 1021, 1275, 1553, under section 5's key), `current_schemas`,
`array_position` in both forms, name's comparisons, and `pg_partitioned_table` and `pg_inherits`, generated by 19's own genbki.pl and served empty. Three grammar rules
(296 in all): a type named by an unreserved keyword (`text`), a second indirection (`schema.table.column`), a type's `[]`.
docs/design/postgres.md section 4, "What the start-up queries have so far", has the rules.

## Against a PostgreSQL 19.0 server

PostgreSQL 19.0 (Debian 19.0-1.pgdg13+1), the postgres:19 image on adacachy2, containers of the lane's own, removed as each run ended.

**Messages (T-PG.12.1).** `scripts/postgres-extended-answers.sh` sends the 237 steps of `crates/amphora/tests/catalog/steps.txt` and records the 739 backend
messages they were answered with (answers.txt, md5 2e38892bdb03a414f78eb6e8f60b8e75: 88 RowDescription, 113 DataRow, 115 CommandComplete, 122 ErrorResponse with SQLSTATE,
message and position, 237 ReadyForQuery, 31 ParseComplete, 26 BindComplete, 7 ParameterDescription). The start-up statements are among them as pgbench sends them, without its tables
(the count's `42P01`, the partition query's no row) and with them (the count, and the partition query's `(2, NULL, 0)`), and by the extended protocol in text
and in binary. T-PG.12.1 sends the same bytes to the wire core, whole frames and then a byte at a time, and every message is the same.

**pgbench 19's built-in scripts** (`PARTS=builtin scripts/postgres-clients.sh` on the binary of bec17c67: exit 0). `pgbench -b tpcb-like` without its tables prints the
same four lines to stderr on both. After `pgbench -i -I dtgvp` (the same output and rows on both), each of `-b tpcb-like`, `simple-update` and `select-only`, at `-M simple`, `extended` and
`prepared`, `-c 1` and `-c 8`, `-t 100 --random-seed=42`, with the vacuum and the truncate it starts by: 18 runs on each server, no failed
transaction in any (100/100 or 800/800), pgbench's output the same but its times, and the four tables row for row the same after each (18 outputs and 72 tables compared).
**Milestone one at one client:** `tpcb-like -c 1` moves the accounts', tellers' and branches' balances by 75966 each, and the history's rows add up to 75966, on both
servers, in each of the three modes; at `-c 8`, 27528 in each; `simple-update` moves the accounts' alone, by the same; `select-only` none.

## Reds on the way

| Tree | What T-PG.12.1 said |
|---|---|
| 0b8eb22f, 12:51Z, its first run | 58 of 221 steps wrong. Then two grammar rules were ported (`text` as a type's name; `schema.table.column`) and two steps that needed unported ones rewritten (an UPDATE without WHERE; a whole-row reference) |
| 781d5a78, 12:53Z | 1 of 221 wrong: an array bound out of integer range is `54000`, where the lane had `2202E` |
| f58b336c (221 steps), d616bf28 and e0c9236d (237 steps) | passed |

## Runs

All on adacachy2, `sync: false`, the GitHub clone (`~/.cache/amphora-perf/postgres-remote`), CARGO_HOME of the lane's, CARGO_TARGET_DIR
~/.cache/amphora-perf-target/postgres, CARGO_INCREMENTAL=0, nice -n 10.

| Run | Tree | Command | Result |
|---|---|---|---|
| 12:47Z, cpus=6 | c4838895 | `cargo check -p amphora-pg-catalog -p amphora-pg-analysis` | no error |
| r2225, 12:47Z, cpus=8 | c4838895 | the two crates' tests; build | passed; 0 |
| 12:51Z, cpus=8 | (none) | the capture | refused for room (7 cores idle), asked again with cpus=4 |
| 12:51Z, cpus=4 | 0b8eb22f | the capture of 221 steps; T-PG.12.1 | exit 0, 1122 lines; RED: 58 wrong |
| 12:53Z, cpus=4 | 781d5a78 | the same | exit 0 (md5 74946fc4); RED: 1 wrong |
| r2248, 12:54Z to 13:04Z, cpus=4, load 18 to 28 | bec17c67 | build; `PARTS=builtin scripts/postgres-clients.sh` | exit 0, every comparison the same |
| r2252, 12:55Z, cpus=6 | 94601326 | fmt; clippy of the five crates | fmt 0; clippy 101 (a method's name) |
| r2256, 12:56Z, cpus=6 | f58b336c | clippy; postgres_isolation, postgres_listener and postgres_protocol; pg-analysis, pg-parser, pg-catalog | 0; 3, 11 and 17 passed (`Compiling amphora-engine`); passed |
| 12:58Z, cpus=4 | d616bf28 | the capture of 237 steps; T-PG.12.1 | exit 0, 1213 lines; passed |
| r2271, 13:00Z to 13:03Z, cpus=6, load 30 | e0c9236d, main 6683e18d merged | fmt; test_requirements; clippy of the five crates; the three suites; the pg crates; build; check-production; `amphora-engine --lib dialect::` | fmt 0; 0; clippy 0 (`Checking amphora-engine`); 3, 11 and 17 passed; passed; 0; 0 (10505, as the ratchet holds); 30 passed |
