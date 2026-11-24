# Lane C21 (expr), 2026-11-24 10:40:25Z to 11:40:25Z: what a column read through something else keeps: the oracles' findings CQ, CP, CX, CR and DE

Machine: adacachy2 (runner 00aad8da, Linux x86_64, 32 cores, shared: 1-minute load 25 to 45 all hour). Every build debug, `--features simulator`, `nice -n 10`, under `timeout`. Base main 08b0be2e, landed as main
7a337181 (11:21:34Z). SQLite 3.54.0, check-in be8d059e9a49 (adacachy's and adacachy2's system library, through Python and the shell). Nothing was timed. Runs named (r...) are in kiln's output log.

## 1. Losses first

- **CR was wrong at default settings on main, on three plain tables of a few hundred rows**, not only under the test setting its finding named: `SELECT count(*) FROM o WHERE NOT EXISTS (SELECT 1 FROM e WHERE
  e.v = o.v AND e.k IN (SELECT p.k FROM p WHERE o.v = p.v))` (300, 60 and 400 rows): SQLite 300, Amphora 0, at default, parallel, statistics and both; and 23 of the 900 small queries drawn differ at default. Fixed; held at all five.
- **DS, found on the way and left** (findings.md): a virtual table's rowid 0 reads NULL (`op_row_id`: `(rowid != 0).then_some(rowid)`), so json_tree's root (rowid 0) comes out again as an unmatched row of a RIGHT or FULL
  JOIN, and `SELECT rowid FROM json_tree(...)` gives NULL for it. json_tree is drawn right of no RIGHT or FULL JOIN.
- **CP is decided as SQLite answers, where the document can be read two ways.** json1.html 4.10 writes `key ANY, value ANY, atom ANY`; datatype3.html 3.1's rule 5 gives a declared ANY NUMERIC affinity (Amphora's answer until now),
  stricttables.html's ANY takes any value as it is (SQLite's answer: it declares no type). The page's other columns (type, fullkey, path, root TEXT; id, parent INTEGER) are kept as it declares them; SQLite declares none, so `j.id = '1'` is 1
  in Amphora and 0 in SQLite. They are not drawn.
- generate_series is not touched. Its `value` is INTEGER in Amphora; SQLite's is an extension that declares none, and SQLite's shell answers by plan (`g.value = '2'` 1 row, `+g.value = '2'` 0 rows).
- new.c and old.c have no affinity, as in SQLite (`new.n = '1'`, n INTEGER, is 0), where datatype3.html 3.2 says a reference to a column value has the column's. The engine's note (SQLite's forum post 819f2d6627) says none; not changed.
- Refused by Amphora and not drawn, none of these causes: a subquery in an UPSERT's DO UPDATE ("Scalar subquery is not supported in this context", 19 of the 22 shapes asked), a subquery in a window function's argument (CS).
- The sweep compares rows as a multiset; a bare column's value, a group's text and a tie's value are not drawn. CX's databases draw what a trigger wrote, not the order two triggers of one event fired in.
- Not run: macOS (one run asked, refused: 0 of 18 cores idle), the whole workspace's tests, `planner`, the corpus subset, a benchmark. A trigger's, an aggregate's and an unnested query's cost were not measured.
- Lost time: four runs refused for room (8, 4, 8 on macOS, and 1 cores asked); the first red run ran on the wrong tree (its branch had not been pushed), and the second took its place.

## 2. Reproduced, smallest (SQLite 3.54.0 / Amphora main; default, parallel, statistics, both, and hash joins only)

| finding | statement | SQLite | main | fixed |
|---|---|---|---|---|
| CQ | `a FULL JOIN json_each((SELECT t.j FROM t WHERE t.k = a.k)) AS je ON je.value = 7`, t `(1,'[7]'), (2,'[8]')` | `1,7; 2,NULL` | `1,NULL; 2,7` (all five) | SQLite's |
| CQ | `z FULL JOIN json_each((SELECT json_array(max(t.k), count(*)) FROM t WHERE t.k IN (SELECT r.x FROM r JOIN l ON r.x = l.y) OR t.k IS z.k)) AS je`, z empty | `NULL,2; NULL,2` | `NULL,1; NULL,NULL` (default, parallel, hash joins only); 120 drawn panic, "cursor id 0 is None" | SQLite's |
| CP | `p, json_each('[5,6,7]') AS j WHERE p.s = j.key`, p.s TEXT '1' | no row | `'1',1` (all five) | SQLite's |
| CX | `WHEN new.name = 'admin'`, NOCASE, 'ADMIN' | fires | never (all five) | SQLite's |
| CR | `o WHERE NOT EXISTS (SELECT 1 FROM e WHERE e.k IN (SELECT p.s FROM p WHERE o.v = p.v))`, e empty, 3 rows of o | 3 rows | 0 (hash joins only, merge joins only; section 1's at default too) | SQLite's |
| DE | `SELECT b.c0 IN (SELECT s.c0 FROM t0 AS s WHERE s.c1 == b.c0), max(quote(b.c1 IS 'abc')) FROM t0 AS b GROUP BY 1`, c1 NOCASE | `0,1; 1,1` | `0,1; 1,0` (all five) | SQLite's |

C10's and C11's own statements (CR's of seed z1, DE's of diff-ae362c4fee7a40c3 by its form) are among the tests; DE's finding file is not in C11's folder, so its database was made anew.

## 3. Traced: the first wrong place of each

- **CQ.** Main's bytecode: `7 Copy r[1] -> r[7]` and `8 VFilter` (the argument read) come before the subquery that writes r[1], `18 BeginSubrtn .. 28 Copy r[13] -> r[1]`, which stands in the row body. `plan_correlated_subqueries` (optimizer/mod.rs)
  holds a subquery of a term that a RIGHT or FULL JOIN's unmatched rows run again to the join's right table, but not for the table's own ON terms. A table function's argument is a term of its own table (`WhereTermOrigin::TableFunction`), read before
  its loop, and was not left out. The second half: `45..57` code the subquery again for the unmatched rows' read (EXPLAIN QUERY PLAN: CORRELATED SCALAR SUBQUERY 1 and 2), and the copy's inner hash join is the first coding's, by its tables' ids.
- **CP.** `JsonVirtualTable::sql` (json/vtab.rs) declares `key ANY, value ANY, atom ANY`: `Affinity::affinity("ANY")` is NUMERIC, so the comparison is by NUMERIC (the finding's "as TEXT" was its look, not its cause). Registered in the SQLite dialect alone (dialect/sqlite.rs).
- **CX.** `variable_from_parameter_index` (trigger_exec.rs) makes a body's `new.c` an `Expr::Variable` with a type's name, the WHEN's rewrite an `Expr::Register`; neither carries a collation, and `register_collations`, the upsert's side channel for it, is emptied around a trigger and by a subquery's
  `Resolver::fork`. The optimizer's readers (a seek, an unnesting, IN's index) read the expression alone. `register_affinities` gave the WHEN's rowid its INTEGER; a body's had none (`new.id = '1'` 0 for SQLite's 1).
- **CR.** EXPLAIN QUERY PLAN, main, hash joins only: `SCAN o, SCAN e, SEARCH p USING COVERING INDEX`: three loops, and its bytecode's `54 Goto 27; anti-join: no match, emit outer row` enters p's semi join, whose `51 Goto 55; semi-join: no match, skip outer row` drops the row. `reads_semi_or_anti_joined_table`
  (optimizer/unnest.rs), which keeps a subquery of a joined table nested, read the subquery's outer columns (`o.v`), not the term it stands in (`e.k IN ..`). Fixed: the plan is `SCAN o, SCAN e, CORRELATED LIST SUBQUERY 1 / SCAN p`.
- **DE.** `47 Column 3 3 -> r[25]`, `50 Eq r[28] r[29]` with no collation (SQLite's `Ne .. NOCASE`): `group_by_process_single_group` (group_by.rs) caches the columns it reads from the sorted row with collation None. window.rs `emit_window_arg_from_cursor` did the same. A GROUP BY that hashes reads the table's cursor, so only a sorted one lost it.

## 4. Fixed

- CQ (ff41b8dc): an argument's term is left out as an ON term is; its correlated subquery is coded once and run at each read of the argument, the unmatched rows' read too (docs/design/executor.md, decision 6). The second coding is gone: 290 lines of `main_loop/close.rs`.
- CP (637e3349). CR (2d143204): the guard reads the term too. DE (dec4ba35): both caches take the column's collation (`cache_scalar_expr_reg`).
- CX (1a10b56f): `ast::TriggerColumn` (a column's declared collation, or the rowid) is in the `Expr::Variable` a body reads and in a new `Expr::TriggerRegister` a WHEN reads; every reader of a collation or an affinity finds it there, and `populate_trigger_register_affinities` is gone.

## 5. Held

`bench/sqlite_columns_read.py` (seed C21; its head names every exception), `tests/fixtures/columns_read.tsv`, `tests/columns_read.rs`: 4,840 queries SQLite answered, in 168 databases, with and without indexes, 27,800 answers.

- CQ: 700 queries, 16 argument shapes (scalar, EXISTS, IN, NOT IN, a row value's IN, aggregates, coalesce, CASE, nested uncorrelated hash joins, none), JOIN, LEFT, RIGHT and FULL, two functions, a join before and after, aggregates, GROUP BY, DISTINCT, windows, in a subquery.
- CP: 1,400 queries, key, value, atom and pragma_table_info's columns against a column of INTEGER, REAL, TEXT, NUMERIC, BLOB, none, NOCASE and RTRIM and its casts and COLLATEs, 8 operators, IN, NOT IN, BETWEEN, row values, CASE, nullif, max, joins, LEFT JOIN, EXISTS, CTEs, derived tables, GROUP BY.
- CX: 160 databases (rowid, STRICT and WITHOUT ROWID tables), 8 triggers each (BEFORE, AFTER, UPDATE OF, DELETE, WHEN), over 9 columns, the id and the rowid, 28 forms of a test, bodies that INSERT, INSERT .. SELECT, UPDATE, DELETE, REPLACE, IGNORE, UPSERT and RAISE, fired by INSERT, OR REPLACE, OR IGNORE, UPSERT, UPDATE, OR REPLACE, OR IGNORE, DELETE; what each wrote.
- CR: 900 queries, an IN, NOT IN, EXISTS, scalar or row value test inside an EXISTS, NOT EXISTS, IN, NOT IN or a count, at nine join settings.
- DE: 1,200 queries, 14 aggregates and FILTERs over 14 forms of a test of 6 columns, grouped by 9 kinds of key (7 subqueries among them), HAVING, ORDER BY, joins, derived tables, window functions.

| `columns_read`, answers that differ from SQLite's | default | parallel | statistics | both | hash joins only |
|---|---:|---:|---:|---:|---:|
| CQ, main's engine (837071af), of 700 (120 of the 699 panics) | 140 | 140 | 139 | 139 | 141 |
| CP, of 1,400 | 591 | 591 | 559 | 559 | 591 |
| CX, of 640 | 146 | 146 | 146 | 146 | 146 |
| CR, of 900 (and 67 merge joins only, 65 statistics and hash joins, 26 and 15 nested loops only) | 23 | 23 | 2 | 2 | 67 |
| DE, of 1,200 | 78 | 78 | 77 | 77 | 78 |
| **the fixed engine, each class** | **0** | **0** | **0** | **0** | **0** |

Red (the tests on main 837071af's engine, tree 6d28bf92, r1951): the five sweeps and the five hand tests of `tests/oracle_findings.rs` failed, the other 92 passed. Green: section 6.

Three more draws of the same size (seeds S1, S2, S3: 72,600 answers, engine 40e52895, r1965) differ in what is not these causes:

- S3, hash joins only, on main too, open (findings.md, DT): `SELECT a0.k, a0.v, a1.k, a1.v FROM p AS a0 LEFT JOIN w AS a1 ON a0.v = a1.k WHERE EXISTS (SELECT 1 FROM n AS s1 WHERE NOT EXISTS (SELECT 1 FROM p AS s2 WHERE a1.v < s2.k AND s2.k = s1.k)) AND NOT EXISTS
  (SELECT 1 FROM e AS s9 WHERE s9.k = a1.v)`: SQLite 5 rows, Amphora 4. Its plan: the EXISTS a semi join to n, the second NOT EXISTS a grouped left join after it, the nested NOT EXISTS still a subquery. Not traced further.
- S2: two databases (rows50, rows132) are refused as a row is inserted ("insufficient registers allocated for expression vector write"), as C11 record has it for row values. Inferred for rows50 from its one insert trigger with a row value beside a scalar subquery, `(lower(new.r), new.tn) < ((SELECT ..), 'b')`; rows132 was not read.
- S1: one tie's value (`max(t.nc) = 'abc'`, 'ABC' and 'abc' in one group), which no rule decides.

## 6. Checks

Linux, adacachy2. On 40e52895, the tree landed but for this record and records of lanes C18 and B27 (main 837071af, then edc3b885, merged in; `crates/` is the same): `cargo fmt --all --check` 0; clippy `-p amphora-engine -p amphora-engine-parser --all-targets --locked
-D warnings` 0 ("Checking amphora-engine", 35 s); check-production 0 ("engine-ratchet: 10581 left .. as the ratchet holds"; edc3b885's file holds 10,591: panic 197 to 195, expect 855 to 849, indexing 3042 to 3040) (r1954). `columns_read` 5 passed (27,800 answers, 44 s at 6 threads), `oracle_findings` 100, `cte_columns` 11, `cte_scopes` 1,
`outer_join_subqueries` 3, `unnesting` 2, `windows_and_triggers` 3 (r1963). r1954's own test step linked the red run's parser (one build folder, two worktrees) and did not compile; r1963 rebuilt every crate.
On 62f0409a (main 837071af in it): those and `on_terms` 5, `in_subquery` 4, `join_methods` 4 (1 ignored), `adaptive_joins` 23, `hash_semi_join` 1, `hash_group_by` 5, `dml_rows` 16, `without_rowid_dml` 4, `row_values` 4, `nested_loops` 7, `plan_cache` 7, `top_n` 3, `upsert_conflict_clauses` 1, `upsert_or_clause` 2 (r1927, r1944).

## Reproduce

```sh
python3 bench/sqlite_columns_read.py > crates/engine/tests/fixtures/columns_read.tsv
cargo test -p amphora-engine --features simulator --test columns_read --test oracle_findings
```
