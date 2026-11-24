# Lane C19 (dml), 2026-11-24 10:36:12Z to 11:36:12Z: what Amphora refused and SQLite 3.54.0 answers

Machine: adacachy2 (runner 00aad8da, Linux, 32 cores, 1-minute load 20 to 56 from other lanes). Every build debug (`simulator` feature), `nice -n 10`, under timeouts. Nothing was timed.
Started on main 05fa02be. Reference: SQLite 3.54.0 (Python's, adacachy and adacachy2). "Red" below is main's engine beneath this lane's tests.

## 1. Losses first

- **INSTEAD OF triggers on views (the list's sixth, 12,547 refusals) are not built.** Not started: the hour went to the first five. CREATE TRIGGER still refuses them.
- **The second, triggers on WITHOUT ROWID tables, is lane C17's on main (511dcc2f, landed 11:01Z) and not this lane's.** Both lanes wrote the same thing in the same hour; main's is kept and mine dropped in the merge. My sweep holds it too.
- **A wrong answer found and left, on main before this lane:** `SELECT * FROM (SELECT a, avg(g) OVER (ORDER BY a), min(a) OVER (ORDER BY a DESC) FROM t)` gives two columns, SQLite three: a `*` over a SELECT of two different windows is a column short.
- **`SELECT * FROM pragma_integrity_check` after a statement a constraint ended** is `Internal error: No row available` (a STRICT, WITHOUT ROWID table's refused type; a plain WITHOUT ROWID table's refused NULL, with statistics). `PRAGMA integrity_check` is right. Seen on this lane's tree alone; main not asked.
- Refusals left, met by the sweeps: a correlated subquery that is a compound (`(SELECT n + 1 UNION ALL SELECT n LIMIT 1)`: "compound SELECT queries not supported yet"), and the outer row's `rowid` by its bare name in a subquery
  (`SELECT (SELECT rowid) FROM kv`: no such column).
- **UPSERT on a table with a rowid checks NOT NULL and CHECK before its BEFORE UPDATE triggers,** SQLite after them: a trigger that RAISEs never runs. And an UPSERT beside a BEFORE UPDATE trigger and an index on a generated column ends
  "Corrupt database: IdxDelete: no matching index entry" (generated columns are behind a flag production does not set). Both left out of the sweep by name.
- **An unread column is left alone more often than SQLite's.** SQLite evaluates it where it neither flattens nor prunes (a CTE with ORDER BY and LIMIT: `integer overflow`); Amphora answers. No document says it is evaluated. The sweep leaves out what SQLite refuses there.
  Kept evaluated, as in SQLite: DISTINCT, grouped and windowed rows, a CTE stored for several readers. Kept evaluated, not as in SQLite: a column that holds a parameter.
- DO UPDATE's subqueries are still refused on a table with custom-type or array columns. The stored text of a table with both options is `WITHOUT ROWID, STRICT` whichever way it was written (SQLite keeps the text).
- Texts not SQLite's, the refusal its: `SET n = (SELECT 1, 2)` is "1 columns assigned 2 values" ("sub-select returns 2 columns - expected 1"); `sum((SELECT 1, 2)) OVER ()` is "row value misused" (the same one).
- Not run: versioned storage and MVCC, a release build, the Postgres dialect, the corpus. The engine's whole unit suite was not run (section 6).
- The red run of the queries is of their first draw (db209acb); the sweep's last two changes (no CTE holds a failing column; no subquery groups by an outer column) have no red run of their own.
- **Lost time:** 2 runs refused for room, two minutes of a wait loop that matched its own command line, one for a cold macOS machine, a base64 shipment mistyped, a landing branch that first carried older copies of two records (rebuilt before anything was pushed to main), the triggers written twice (above).

## 2. Reproduced, smallest (r1835, main 05fa02be, the same in default, parallel and statistics)

| class, refusals of C11's run | statement | SQLite | main | fixed |
|---|---|---|---|---|
| 1, 7,701 | `CREATE TABLE a(k INT PRIMARY KEY, v TEXT) WITHOUT ROWID, STRICT` (and the other order) | made | near "STRICT": syntax error | made |
| 2, 2,423 | `DELETE FROM t WHERE k = 'a'`, AFTER DELETE trigger, WITHOUT ROWID | the row goes, the trigger logs | refused | C17's |
| 3, 4,045 | `INSERT OR IGNORE INTO t VALUES ('a', 1), (NULL, 2), ('b', 3)`, `t(k TEXT PRIMARY KEY, n INT) STRICT` | a, b | NOT NULL constraint failed: t.k | a, b |
| 4, 79,850 | `SELECT a FROM t UNION SELECT 'C' ORDER BY (1 COLLATE NOCASE)` | A, b, C | 1st ORDER BY term does not match any column in the result set | A, b, C |
| 5 (C14) | `.. ON CONFLICT(id) DO UPDATE SET n = (SELECT excluded.n + 1)` | 8 | Subquery is not supported in this position | 8 |
| 5, CW | `SELECT count(*) FROM v`, v's json_extract(body) on a row that is not JSON | 2 | malformed JSON | 2 |
| 5, CR, CS | `SELECT a, sum((SELECT max(b) FROM s)) OVER (ORDER BY a) FROM t` | 1,2 2,4 | Scalar subquery is not supported in this context | 1,2 2,4 |
| 6, 12,547 | `CREATE TRIGGER g INSTEAD OF INSERT ON w ..` | made | refused | not built |

## 3. Traced to the first wrong step

| what | where | fixed |
|---|---|---|
| STRICT beside WITHOUT ROWID | the parser takes both. The schema stores a CREATE TABLE as its syntax tree writes it (`ast/fmt.rs`), which wrote `WITHOUT ROWID STRICT`; ParseSchema's own parse of that refuses it | the comma |
| a STRICT key's NULL | the schema left the key nullable, and the type check refused its NULL (`check_null_keys`), whatever the OR clause; and it ran before NOT NULL, so a row with a NULL and a wrong type was refused under OR IGNORE, and `NOT NULL DEFAULT '7'` under OR REPLACE stored text in an INT column | the key is NOT NULL in the schema (stricttables.html; sqlite3EndTable), NOT NULL before the types (sqlite3GenerateConstraintChecks), `check_null_keys` gone |
| ORDER BY (1 COLLATE x) | `resolve_compound_order_by_expr` took one COLLATE off and looked for a literal: a parenthesis was neither | COLLATEs and parentheses off first, the outermost COLLATE sorts; a position is what sqlite3ExprIsInteger reads (signs, hexadecimal); the term's ordinal in "out of range" |
| DO UPDATE's subquery | nothing planned it: `emit_upsert` rewrote column names to registers and met `Expr::Subquery` | `DoUpdateSubqueries`: planned with the conflicting row as their outer table (read through its cursor), `excluded.*` as registers; run for each conflict if it reads excluded or that row, once if neither, as SQLite runs them |
| a window's argument | `link_with_window` counts an argument's columns before its subqueries are planned | counted where they are planned, the innermost layer |
| a view's unread column | the view's SELECT is emitted whole | NULL in its place before its plan is made, its own subqueries dropped, the marks on views beneath taken back (`leave_out_unread_columns`) |
| a trigger's failure under OR IGNORE | `op_program` dropped any constraint error of a trigger program when the statement that fired it was OR IGNORE: a STRICT type refused in a trigger went unrefused, and an `INSERT OR IGNORE .. DO UPDATE` whose trigger broke a key went through (C17's record, section 1, left) | the error is the statement's; a trigger's statement under OR IGNORE ignores by itself already |

The last is beside the list. It was met by the sweep of triggers (seed 7, cases 1364 and one more; seed 2, case 808; seed 5, case 2046) and is one arm of a match, removed.

## 4. Held

`bench/sqlite_dml_rows.py COUNT SEED` (lane C14's, widened) draws STRICT beside WITHOUT ROWID in both orders, triggers on WITHOUT ROWID tables, triggers that act (WHEN, RAISE, a write to their own table), NULL in every key, DELETE, UPDATE OR, and subqueries of
DO UPDATE's SET and WHERE (13 forms: excluded, the conflicting row, the table itself, nested, in FROM, a vector's). `fixtures/dml_rows.tsv` is 640 cases of it. Each statement (done or refused), the rows of the table, the triggers' log and `integrity_check`, under default, parallel and statistics.
`bench/sqlite_refused_queries.py COUNT SEED` draws a compound's ORDER BY terms (positions, names and expressions under parentheses, COLLATEs and signs; the order compared), a window's arguments, FILTER, PARTITION BY and ORDER BY with 16 forms of subquery, and views, derived tables
and CTEs with columns that fail on some row, read by 27 forms of query. `fixtures/refused_queries.tsv` is 1,200 cases.
Hand-written: `crates/engine/tests/dml_rows.rs` (3 more), `crates/engine/tests/refused_queries.rs` (2).

| tree | what | runs | wrong |
|---|---|---:|---:|
| main fcad33f7 (red, db209acb, r2016): the fixtures as first committed | dml, 640 cases | 1,920 | 672; 11 of the file's 19 tests fail |
| the same | queries, 1,200 cases | 3,600 | 2,244; 6 of 6 tests fail |
| main 05fa02be (red, r1857): the widened generator before DO UPDATE's subqueries, 420 1 | dml | 1,260 | 369, 33 of them panics (CP) |
| main 05fa02be (r1857): 1500 1 | dml | 4,500 | 1,305, 99 panics |
| the first four fixed (e7947b5a): 1500 1 | dml | 4,500 | 0 |
| the same: 1500 2 and 3000 5 | dml | 13,500 | 24: a trigger's refusal under OR IGNORE (2 cases), the order of a multi-row DELETE (3, no document says), the two rowid-table causes of section 1 |
| with DO UPDATE's subqueries (1adb33d7): 1500 7, 3000 8 | dml | 13,500 | 204: a second conflict read the first's answer (this lane's own error, fixed), correlated compounds (refused anywhere) |
| 4976e9e1: 1500 8 | dml | 4,500 | 0 |
| b0e95e4d (main ed71a93e merged): 1500 9 | dml | 4,500 | 0 |
| bc340ee2: fixture of 1,200 | queries | 3,600 | 126: subqueries in a view beneath a view (fixed), `*` over two windows |
| b0e95e4d: 1500 2, 1500 3 | queries | 9,000 | 51, all one shape: a subquery that groups by its outer row's column (refused on main too; left out since) |

The queries' first landing tree (5ebce39b) failed lane C18's cte_scopes, 16 of 4,800: a CTE's unread column was left out where another use read the stored rows, and answered NULL. It was wrong and is gone: a CTE's columns are evaluated, as SQLite's
disableUnusedSubqueryResultColumns leaves a CTE's alone. Nothing of it reached main.

## 5. What was wrong in this lane's own work, and who caught it

- DO UPDATE's subqueries ran once where they read only `excluded` (the sweep, seeds 7 and 8); a view's subquery ran though its column was left out (the sweep); a CTE's column (cte_scopes, above).
- The first generator asked an undocumented order of a multi-row DELETE; the hand test of a STRICT, WITHOUT ROWID table asked `pragma_integrity_check` after a refused INSERT (section 1's finding): its refused statements now come last.

## 6. Checks

All on adacachy2.

- e795d19a (main fcad33f7, r1943, r1952): fmt 0, clippy -p amphora-engine -p amphora-engine-parser --all-targets 0 ("Checking amphora-engine v0.1.0"), check-production 0 (10,594; cast_sign_loss 233 to 231), dml_rows 19, refused_queries 6, oracle_findings 92,
  upsert_conflict_clauses 1, upsert_or_clause 2, without_rowid_dml 4, without_rowid_affinity 2, row_changes 4, row_values 4, plan_cache 7, outer_join_subqueries 3, windows_and_triggers 3 (lane C17's sweeps), cte_columns 11, in_subquery 4, unnesting 2, hash_semi_join 1,
  planner 14, records_plans 5, plan_hints 3, top_n 3, nested_loops 7, hash_group_by 5, without_rowid_seek 6, without_rowid_analyze 2, without_rowid_untyped_key 1, column_stats 10, join_methods 4, on_terms 5, the parser's 33, all passed.
- 4271227e (main 7a337181, lanes C18 and C21 in it, r1987): fmt 0, clippy 0 (checked), check-production 0 (10,579, as the ratchet holds), dml_rows 19, refused_queries 6, cte_scopes 1 (4,800 answers), oracle_findings 100, upsert_conflict_clauses 1, upsert_or_clause 2, without_rowid_dml 4, windows_and_triggers 3.
- The tree that was pushed, 0d221cb6 (main 0feda2e0, the Postgres listener's unit 10, r2006): fmt 0, clippy 0 (checked), check-production 0 (10,579), dml_rows 19, refused_queries 6, cte_scopes 1. **Landed as 0d221cb6, 11:32:24Z.**

Not run on the pushed tree: the planner's suites (they passed on e795d19a, before lanes C18 and C21), cte_columns (passed on 5ebce39b, before the CTE's columns were taken out of the change), windows_and_triggers (on 4271227e), the engine's unit suite at all.

## 7. Reproduce

```sh
python3 bench/sqlite_dml_rows.py 640 1 > crates/engine/tests/fixtures/dml_rows.tsv                   # SQLite 3.54.0 in Python
python3 bench/sqlite_refused_queries.py 1200 1 > crates/engine/tests/fixtures/refused_queries.tsv
cargo test -p amphora-engine --locked --features simulator --test dml_rows --test refused_queries
# wider: the same tests reading another draw (diag/dml-c19's probe/c19_dml.rs and c19_rq.rs, not for main)
```
