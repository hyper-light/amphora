# Lane C15 (expr), 2026-11-24 09:41:39Z to 10:41:39Z: a row with a NULL is compared by its other fields (the oracles' finding CT), and row values held to SQLite by a sweep

Machine: adacachy2 (runner 00aad8da, Linux, 32 cores, 1-minute load 27 to 101 all hour from other lanes). Every build debug with `simulator`, `nice -n 10`, 2 to 8
jobs. SQLite 3.54.0 (be8d059e, the system's `sqlite3` and the library Python links). Started on main 70f6712b. Runs named (r...) are in kiln's output log.

## 1. Losses first

- **Five other ways a row value answers wrong are still on main**, at their own first wrong places. None is CT's, so none was touched (section 5). By count
  in an unrestricted sweep (2,400 queries, seed 77, default, on the fixed engine): 133 refused (CM's BETWEEN as a value), 5 answered otherwise (4 lists of rows, 1 row subquery).
  A sixth, a field's collation, the sweep found at 09:53Z; lane C16 fixed it at the place traced here (e89b0e9a) while this landed, so the fixture holds it.
- **The fixture leaves those five forms out**, by name (`LEFT_OUT` in bench/sqlite_row_values.py). So BETWEEN is held in a WHERE, an ON, an EXISTS and a correlated subquery,
  not in a select list, and a row subquery is held only beside plain columns of its own names (29 of 1,500 queries).
- **The sweep never met DB** (a range of a row of two tables): seed 77's 34 queries of the form all answered as SQLite. `LEFT_OUT` drops the form from the fixture anyway. DB's plain
  form still gives 8 rows for SQLite's 4 (section 5).
- **The documents' own model test was blind.** `in_subquery.rs` "row values with NULLs in either place" binds its left sides as parameters, which are computed once, before the loop,
  so no field was ever skipped. It passed with CT on main. It is left as it is; `row_values.rs` reads columns.
- CT's 323 findings are not replayed: the record keeps two with their statements (both answer as SQLite now, four settings); the other 321 are in `findings.tsv` as queries without
  their databases. They "may hide other causes" (C11): the sweep found six others, and an oracle run on the fixed engine may find more. Not run: no room for the bench's build in the hour.
- SQLite contradicts itself on a row IN a list it reads from an index `r DESC, i` (`USING INDEX .. FOR IN-OPERATOR`): 10 of the 7,200 queries of seeds 77, 101 and 102. It answers 0 where the same query
  without the index answers NULL, as Amphora does and rowvalue.html 2.1 says. The generator asks SQLite both ways and writes no query whose answers differ. Not reduced.
- Not run: clippy for macOS and Windows, the engine's whole unit suite, a release build, a benchmark (none was due).
- Lost time: two runs refused for room (7 and 2 of 32 cores idle); a plan check of mine asked the keyed tables for a seek by a row they never take, so one build's `keyed` test failed on it
  with 0 wrong answers (r1677); two generator checks whose output I redirected wrongly; main's engine moved four times as this landed (lanes C13, C16, C12 with PG22, C14), each merged and checked again: the fix was green at 10:07Z and is on main as 75c5e509 from 10:35:51Z.

## 2. Reproduced

`o(id, region TEXT, code INT)`: (1, 'eu', 7), (2, NULL, 8), (3, 'us', 9), (4, NULL, 7); `closed(region TEXT, code INT)`: ('eu', 7). Main 70f6712b (r1610), the same at default, parallel, statistics and both:

| statement | SQLite | Amphora |
|---|---|---|
| `SELECT id, (region, code) IN (SELECT region, code FROM closed) FROM o` | 1, 0, 0, NULL | 1, NULL, 0, 0 |
| `SELECT id FROM o WHERE (region, code) NOT IN (SELECT region, code FROM closed)` | 2, 3 | 3, 4 |
| the first, `WHERE id = 2` (the smallest) | 0 | NULL |
| the first, `ORDER BY id DESC` | 2: 0, 4: NULL | 2: 0, 4: NULL |
| `(code, region) IN (SELECT code, region FROM closed)` | 1, 0, 0, NULL | 1, 0, 0, NULL |

The program (EXPLAIN, default; the same at the other three):

```
16 Integer   r[6]=0
17 Column    r[7]=o.region
18 IsNull    r[7] -> 24        the first NULL leaves here: r[8] is not computed, r[7..8] not converted
19 Column    r[8]=o.code
20 IsNull    r[8] -> 24
21 Affinity  r[7..8] A, C
22 NotFound  r[7..8] -> 24
24 Rewind    list
26 Ne        r[7], list.region -> 30
28 Ne        r[8], list.code   -> 30     r[8] is the row before's code, or nothing on the first row
29 Goto      NULL
```

SQLite 3.54.0's: Column, Column, Affinity, IsNull, IsNull, Found, Rewind.

## 3. Traced

The mechanism predicts each row, and each prediction was run before the fix (adacachy2, 09:48:31Z, tree 70f6712b): row 2 alone reads a register nothing wrote (NULL: the answer is NULL, SQLite 0);
row 2 after row 1 reads row 1's 7 (NULL); row 4 after row 3 reads row 3's 9 (0, SQLite NULL); descending, row 2 reads row 3's 9 (0, right by luck) and row 4 nothing (NULL, right by luck); with the
fields the other way round the NULL is last, all before it are computed, and the answers are SQLite's. All five came out so. `(NULL, abs(-9223372036854775808)) IN (SELECT 1, 2)` gave NULL for SQLite's
"integer overflow": the field was never computed.

The first wrong place is `translate_subquery_result_expr` (crates/engine/translate/expr/translator.rs): the NULL test of a field stood inside the loop that computes the fields. `translate_in_subquery_condition`
(expr/condition.rs) has the same loop: its rows were right (a NULL fails a condition), but a later field's error was hidden there too. Not the planner's: a row value IN stays a subquery (`try_rewrite_in` takes one column), and no
index seek applies it.

## 4. Fixed, and held

`translate_in_subquery_lhs`, which both now call: every field computed, the row converted by the list's affinity, then each field tested for NULL, in SQLite's order. No other code changed (+78 -71 lines with the
decision in docs/design/executor.md section 2).

`crates/engine/tests/row_values.rs`, four tests at four settings; `bench/sqlite_row_values.py` writes the fixture with SQLite's answers.

| | queries | red: main's engine, wrong at default / parallel / statistics / both (r1677, tree 4cf8d979) | green (r1701, tree b2e0f010) |
|---|---:|---|---|
| hand-written forms | 16 and 2 errors | fails at the first | pass |
| without indexes | 500 | 35 / 35 / 36 / 36 | 0 / 0 / 0 / 0 |
| seven indexes of rows | 500 | 37 / 37 / 37 / 37 | 0 / 0 / 0 / 0 |
| a WITHOUT ROWID key of a row, partial and unique indexes | 500 | 42 / 42 / 43 / 43 | 0 / 0 / 0 / 0 |

The red and green counts are of the fixture as first written, less the collation form. As landed, 36 of the 1,500 hold that form, and all answer as SQLite (r1796, tree f10cec90). What the 1,500 hold: 620 IN or NOT IN
(SELECT ...) (correlated lists, UNION ALL and LIMIT 0, 1, 3 among them), 177 IN lists, 191 BETWEEN, 483 of = <> < <= > >= IS IS NOT, 29 row subqueries; as a value 177, in a WHERE 188, a NOT 87, IS NULL 93, OR 110, AND 113, CASE 96,
an aggregate 76, pairs 78, JOIN 106, LEFT JOIN 85, ON and WHERE 96, EXISTS 96, a correlated count 99. As values alone, in the first fixture, SQLite's IN (SELECT) is 1 in 277 rows, 0 in 3,194, NULL in 2,444; NOT IN 4,767, 545, 2,047.

More seeds on the fixed engine (r1677, 2,400 queries each, four settings): seed 101, 0 wrong; seed 102, 6 wrong of 800 in `indexed`, every one SQLite's contradiction above.

## 5. What else the sweep found: other first wrong places, not touched

| class | smallest form: SQLite / Amphora (the fixed tree, adacachy2 10:06:09Z, four settings) | first wrong place |
|---|---|---|
| CM, BETWEEN as a value | `SELECT ('a', 1) BETWEEN ('a', 0) AND ('a', 2)`: 1 / refused | `translate_between_expr` (expr/metadata.rs) takes one register for the row on the left. 133 of 2,400 |
| CM, a constant call in a row | `SELECT (id, 6) < (1, abs(-7)) FROM o`: 0 / refused | `translate_constant_call_once` (expr/translator.rs) keeps and copies one register for a row that is constant and calls a function |
| DB, a range of a row of two tables | `e JOIN f ON ((e.day, f.hour) BETWEEN (1, 10) AND (2, 4))`: 4 / 8 at default and parallel, 4 with statistics | `autoindex_prefilter` (optimizer/mod.rs): `constraint.lhs_mask` empty is read as "the term reads no other table", but a row range's mask (`row_value::constraint`) is its bound's only. The index over e is built once, its rows tested against f's first row |
| a field's collation, fixed by lane C16 (e89b0e9a) | `SELECT (x, 1) = (d, 1), (x, 1) <> (d, 1), (x, 1) IN ((d, 1))` over `x TEXT`, `d TEXT COLLATE RTRIM`, ('a ', 'a'): 0, 1, 0 / 1, 0, 1 before it (`x = d` is 0 on both) | `resolve_comparison_collseq_with_resolver` (collate.rs): a left column that declares no collation counts as none, so the right's is used; its siblings use `comparison_operand_column_collseq`, where it is BINARY. Its caller: `comparison_collation`, for every row comparison |
| new: a row subquery's fields | `(lower(t0.n), t0.c) < (SELECT q.n, q.c FROM s q WHERE q.id = 7)`: 1 / 0 for two rows; the scalar `lower(t0.n) < (SELECT q.n ...)` is right | `row_component_affinity_collation` (binary.rs), its own TODO: a side that is a row subquery has no component, so no affinity or collation |
| new: a scalar subquery in an IN subquery's columns (a scalar IN too) | `t0.c IN (SELECT (SELECT m.r FROM s m WHERE m.id = q.id) FROM t q)`, c TEXT, r REAL: 9 rows are 1 / 3 | not traced: the list's affinity is not REAL's |
| no document says | a list of several rows with columns, `(t0.c, lower(t0.i)) NOT IN ((t0.c, t0.i), (t0.c, (t0.i + 0)))`: SQLite converts every row by the affinity of one; Amphora by each row's | rowvalue.html 2.1.1 gives a row IN a subquery only; datatype3.html 3.3.1 calls a compound's affinity indeterminate |

CN was not met. CM, CN and DB are lane C11's names (bench/records/2026-11-24-adacachy2-perf-oracle-c11.md); lane C13 calls two other findings CO and CM.

## 6. Checks

Linux, adacachy2. Commits 78cd7b60 (the tests, red) and 208390c7 (the fix) on main b0e68ba7; merged with main b549489e (lane C13's landing) as c82fdc92, with 7d9fcb6b (lane C16's) as f10cec90, each built and checked whole; then dedcd26d (the fixture with the collation form, no Rust) and main
b5584c1e (lanes C12's and PG22's, engine files) as ebc40a7c, the touched tests and the gates again;
last with ac3cf51f (lane C14's, engine files, the ratchet untouched) as 69421041: fmt 0, row_values 4, in_subquery 4, row_value_seek 5, oracle_findings 92, clippy 0 ("Checking amphora-engine v0.1.0", 37 s), at 10:35:38Z. Pushed as 75c5e509; what main moved between was a record.

| | b2e0f010 (r1701) | c82fdc92 (r1777) | f10cec90 (r1796) | ebc40a7c (r1818) |
|---|---|---|---|---|
| `row_values` (0 passed, 4 failed without the fix, r1677) | 4 | 4 | 4; and reading dedcd26d's fixture, 3 of 3 | 4 |
| in_subquery, row_value_seek | 4, 5 | 4, 5 | 4, 5 | 4, 5 |
| oracle_findings, outer_join_subqueries | 86, 1 | 91, 2 | 91, 2 | 92, not run |
| hash_semi_join, unnesting, on_terms | 1, 2, 5 | 1, 2, 5 | not run, 2, 5 | |
| cte_columns (lane C16's) | | | 10 | |
| planner | 14 of 14 | 14 of 14 | 13 of 14: `a_whole_table_group_by_scans_and_sorts..` (planner.rs:182), main's own by lane C9's landing (af8ca01c); not run again alone | |
| records_plans, ephemeral_reuse, plan_cache, join_methods, index_filters | 5, 2, 7, 4, 2 | | | |
| `cargo clippy -p amphora-engine --all-targets --locked -- -D warnings`, "Checking amphora-engine v0.1.0" in each log | 0 | 0 | 0 | 0 |
| `check-production` | 1: two counts fell | 10,613, as the ratchet holds | 0, 10,603 | 0, 10,597 |

The ratchet is lowered with the fix (it takes away an inherited `unwrap` and two unchecked additions): unwrap_used 433 to 432, arithmetic_side_effects by 2. `cargo fmt --all --check` 0; `scripts/test_requirements.py` OK. without_rowid_seek 6 on the fix alone (r1654).
Not finished: the engine's unit tests under `translate::` (built in 65 s, stopped at 10:16Z, still running, for the merged tree's checks).
The same shape elsewhere: every other `IsNull` in a loop over fields was read (seek.rs, adaptive.rs, fkeys.rs, without_rowid.rs): each jumps past all that reads the fields, or stands after the loop that computes them.

## 7. Reproduce

```sh
python3 bench/sqlite_row_values.py > crates/engine/tests/fixtures/row_values.tsv      # SQLite 3.54.0's answers
cargo test -p amphora-engine --features simulator --test row_values
python3 bench/sqlite_row_values.py 800 77 all                                          # the forms left out too
```
