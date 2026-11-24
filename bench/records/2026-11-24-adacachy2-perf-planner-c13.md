# Lane C13 (planner): oracle findings CO and CM, a CTE an ON clause's subquery reads and a WITHOUT ROWID table right of a RIGHT or FULL JOIN (2026-11-24)

Hour: 09:29:09Z to 10:29:09Z. Machine: adacachy2 (Linux x86_64, 32 cores, shared: 1-minute load 18 to 92), debug builds (`--features
simulator`); SQLite 3.54.0 is the reference (adacachy's system library, 2026-10-09). No timing is in this record.

Losses first.

- **The sweep's queries still find one defect**, on main before, in WHERE as in ON, under a WITH inside another: DI, a CTE that only an unread CTE
  of an inner WITH names is planned, and its error raised: SQLite 1 row, Amphora "no such table". (DH, the second, landed after: below.)
- The drawn fixture that lands (1,000 queries, 5,000 answers) holds no query of DH or DI; seed w1 of 300 queries a database holds one of each (2 of 1,500).
- DF and DG are fixed beyond CO and CM: the sweep of CO's queries could not answer without them.
- An IN over a bare table name (`x IN c`) is not supported in Amphora at all (not new, not touched).
- Two uses of one outer CTE, each inside a nested WITH's body, still read copies of one plan (`base_outer_refs_for_cte_planning` keeps the syntax
  back there). No drawn query and no reduction shows it wrong; it is the shape DF had, so it is a suspicion (not a finding).

## Reproduced (done when 1), on main 7173ca16, at default, parallel, hash joins only and nested loops only alike

```sql
CREATE TABLE o(x INT); INSERT INTO o VALUES (1), (2);   CREATE TABLE r(x INT); INSERT INTO r VALUES (1), (5);
WITH c AS (SELECT x FROM r) SELECT a.x, b.x FROM o AS a JOIN o AS b ON a.x IN (SELECT c.x FROM c);  -- SQLite 1,1; 1,2. Amphora "no such table: c"
WITH c AS (SELECT x FROM r) SELECT j.value FROM json_each((SELECT json_array(x, 3) FROM c LIMIT 1)) AS j;  -- SQLite 1; 3. Amphora "no such table: c"
WITH l AS (SELECT x FROM r) SELECT a.x, b.x FROM o AS a JOIN o AS b ON a.x IN (SELECT x FROM l);  -- with a table l: main read the table (inferred from the code; in the test)
CREATE TABLE e(k INT); INSERT INTO e VALUES (1), (2);
CREATE TABLE w(k INT PRIMARY KEY, v INT) WITHOUT ROWID; CREATE INDEX w_v ON w(v); INSERT INTO w VALUES (1, 1), (6, 7);
SELECT count(*) FROM e AS a0 FULL JOIN w AS a2 ON a0.k = a2.v;   -- SQLite 3. Amphora "secondary index lacks primary key locator" (with statistics: 3)
SELECT a0.k, a2.v FROM e AS a0 FULL JOIN w AS a2 ON a0.k = a2.v;   -- the same: the index covers what the query reads. With a2.k and a2.v read: right
```

CM errors where the unmatched rows are read through the secondary index: a covering one by default, and any that a WHERE term seeks or INDEXED BY names.

## Traced (done when 2)

- **CO.** `parse_from` (`translate/planner.rs`) plans the CTEs that subqueries read before it parses the FROM clause, so that their FROM clauses find
  them, by the names that the query's other clauses hold (`collect_table_refs_outside_from`, and the SET, WHERE and RETURNING of an UPDATE). The
  FROM clause's own expressions were in neither list. Red: 1,400 of the fixture's 5,000 answers "no such table". `collect_subquery_table_refs_in_from_exprs` reads
  its ON clauses, its table functions' arguments, and those of its parenthesized joins.
- **CM.** `emit_unmatched_right_rows` (`main_loop/close.rs`) allocated its index cursor with the index as declared. `emit_deferred_seek` finds a WITHOUT ROWID
  row by the primary-key columns its index's entries end in, which `Index::physical_index` adds and `JoinedTable::open_cursors` gives the loops' cursor.
  `JoinedTable::index_of_cursor` gives both that index now.
- **DF** (found by the sweep of CO's queries, on main for WHERE too). `WITH c AS (SELECT k FROM q), d AS (SELECT k FROM c) SELECT c.k FROM c WHERE (SELECT
  count(*) FROM d) = 3`: SQLite 3 rows, Amphora 1; over a recursive c a panic, "cursor id 0 is None" (`vdbe/mod.rs:1607`). The program: the loop's coroutine opens
  pseudo-cursor 0, the subquery's code opens it again (`OpenPseudo 0`) and closes it (`Close 0`). In a subquery's FROM the name finds an
  entry of the outer query's references: the first with the name, which is the outer FROM clause's use, whose table holds the plan that use
  reads (a CTE's syntax is kept on the WITH clause's entry, and not for a recursive CTE). The copy kept its table IDs, so its cursors, hash
  tables and Once blocks. Now the WITH clause's entry holds every CTE's syntax, a read finds that entry by the CTE's id, and a recursive CTE is planned anew too.
- **DG** (found the same way). `mark_outer_query_ref_cte_definition_only` hid the first outer entry of the CTE's name, a table's among them.
- **DH** (found the same way, on main for WHERE too). `find_cte_outer_query_ref_by_identifier` took the first of the outer references with the name,
  the outermost; an inner WITH's entries are added after. It takes the innermost definition's, the last on a tie. A CTE's body is planned where it is
  read, so it could meet an inner WITH's entry of a name it reads: `cte_body_refs` gives it only those its own WITH clause sees.

## Held by tests (done when 3)

- `tests/oracle_findings.rs`, six tests: 22 statements for CO, an INSERT and an UPDATE FROM, 18 for CM, 12 for DF, 3 for DG, 8 for DH; six settings (default,
  parallel, statistics, both, hash joins only, nested loops only).
- `on_clauses_over_ctes_and_joins_of_keyed_tables_answer_as_sqlite` (`tests/outer_join_subqueries.rs`): `bench/sqlite_on_ctes.py` draws, and asks SQLite, 200
  queries in each of five databases: ON clauses with subqueries over CTEs (plain, over another CTE, recursive, a compound, aggregated, LIMITed, with a join of
  their own; a WITH of the subquery's own; MATERIALIZED or not; a bare CTE in FROM, a derived table, a parenthesized join, a compound, a table function's
  argument, an EXISTS around the whole) in LEFT, RIGHT, FULL, inner and cross joins, without and with indexes; and WITHOUT ROWID tables left and right of
  each join, with no index, with secondary indexes (one by a prefix, one DESC, one under another collation, unique, over a composite NOCASE key) and
  with partial and expression ones. Five settings; every table is read after each query and holds what it held.

| run | tree | answers | differ | what |
|---|---|---:|---:|---|
| the landed fixture | aeea6a47: the tests, main's engine (07d95147) | 5,000 | 1,455 | 1,400 "no such table" (CO), 45 CM, 10 DG; `oracle_findings` 86 passed, 5 failed |
| the fixture's first draw, no bare CTE | b6db4e91: CO and CM alone | 5,000 | 5 | panics, "cursor id 0 is None" (DF) |
| drawn, seeds w1, w2, w3, w5, w6 (the same first draw) | b6db4e91 | 37,500 | 85 | panics, "cursor id N is None" (DF); no error, no wrong answer |
| drawn, seeds w1 to w4 | ecbb33e1: DF but its last form, and DG | 30,000 | 12 | 5 errors (DI), 5 wrong (DH), 2 wrong (DF's last form: a nested WITH) |
| the landed fixture | 82cd9499, 1c941245 (landed) | 5,000 | 0 | `oracle_findings` 91 passed |
| drawn, seeds w1, w2, w3, w4, w7, w8 | 82cd9499: the landed engine | 45,000 | 10 | 5 errors (one query, DI), 5 wrong (one query, DH), both in w1 |
| the landed fixture and C10's (3,000) | fedc9015: DH too, landed after | 8,000 | 0 | `oracle_findings` 92 passed; the wide seeds were not run again |

No panic, and no table changed, in any run of the landed engine. C10's sweep (3,000 answers) differs in none.

## Checks (Linux, adacachy2, 1c941245: the landed engine and tests)

- clippy `-p amphora-engine --all-targets` 0 ("Checking amphora-engine", 44 s); `cargo fmt --all --check` 0; check-production 0 (10,616, as the ratchet holds).
- oracle_findings 91, outer_join_subqueries 2, on_terms 5 (lane C9's sweep), in_subquery 4, join_methods 4 (1 ignored), adaptive_joins 23, hash_semi_join 1,
  unnesting 2 (simulator), planner 14 of 14, without_rowid_seek 6, without_rowid_dml 4, without_rowid_affinity 2, lib `subquery` 9, lib `cte` 56, C6's small shards 4
  of 4, lib `optimizer::` 178 of 179 (without T-PL.8.13's other shards): the run was stopped for room in its last test,
  `t_pl_8_14_ab_left_join_probes_change_bushy_plans_only_where_cheaper`, which had run over a minute with no failure.
- Red, on aeea6a47 (the tests, main's engine): the five tests of `oracle_findings` fail (CO and DG "no such table: c" and "no such column: c.k", CM "secondary index lacks primary key
  locator", DF the rows of the plain CTE), and the sweep as in the table. The three statements of DF's last form were run on main's engine by hand: `(WITH x AS
  (SELECT k FROM d) SELECT count(*) FROM x) = 3` 1 row for SQLite's 3, the same over a recursive d a panic, and as an ON term 0 rows for 2.
- DH (fedc9015: the first landing b549489e and DH's test and fix): clippy 0 (checked, 54 s), fmt 0, check-production 0 (10,616), oracle_findings 92, outer_join_subqueries 2
  (8,000 answers), in_subquery 4, join_methods 4 (1 ignored), adaptive_joins 23, on_terms 5, planner 13 of 14: `planner.rs:182` (a whole-table GROUP BY, no CTE)
  failed as lanes C4, C9 and C10 recorded of main, and passed on 1c941245. Lib suites, hash_semi_join, unnesting and the wide seeds were not run on it.
- The tree of the first landing differs from 1c941245 in crates/amphora, crates/line-loom, docs, scripts and records (main's), which amphora-engine does not depend on (`cargo tree`).

## Reproduce

```sh
python3 bench/sqlite_on_ctes.py 200 CO > crates/engine/tests/fixtures/on_ctes.tsv   # SQLite's answers
cargo test -p amphora-engine --features simulator --test oracle_findings --test outer_join_subqueries
python3 bench/sqlite_on_ctes.py 300 w1 > w1.tsv   # the wide sweep: the same test, its fixture read from this file
```
