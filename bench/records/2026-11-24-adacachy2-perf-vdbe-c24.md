# Lane C24 (vdbe), 2026-11-24 11:24:47Z to 12:24:47Z: a virtual table's row and a semi join's condition: the oracles' findings DS and DT

Machine: adacachy2 (runner 00aad8da, Linux x86_64, 32 cores, shared: 1-minute load 20 to 67 all hour). Every build debug, `--features simulator`, `nice -n 10`, under `timeout`. Base main 20cffc15, landed as main
24992ce6 (12:02:01Z, with lanes C19 and C22 and decision 45 merged in). SQLite 3.54.0, check-in be8d059e9a49 (adacachy's system library through Python; its shell where generate_series is drawn). Nothing was timed.
Runs named (r...) are in kiln's output log.

## 1. Losses first

- **DT was wrong at default settings on main**, over three plain tables of one or two rows (section 2), and in 36 of the 1,500 small queries drawn (34 parallel, 11 with statistics, 12 both, 38 hash joins only).
  Its finding named a test setting (`Connection::set_join_methods`, which only tests and bench/join_methods call: no PRAGMA, no server option); a hash join is in neither plan. A user meets it in a plain SELECT.
- **DS was five defects, not one**, all at default settings: rowid 0 NULL; a NULL row's rowid the last row's; json_each's and json_tree's rowids other numbers than SQLite's; a LEFT or FULL JOIN to
  generate_series that never ended; a rowid read past a materialized hash build (section 3). 870 of the 1,400 queries drawn differed at default, and 3 more never ended.
- json_each's and json_tree's `id` and `parent` are other numbers than SQLite's (its are offsets in the JSONB): left. json1.html 4.10 promises only that id differs row by row.
- The sweep does not draw: generate_series' value or rowid beside text or a real (SQLite reads either as an integer by plan: `json_each('{"a": {"b": 1}}') AS t0 LEFT JOIN generate_series(2, 0, -1) AS t1 ON t1.rowid = t0.value`
  matches the text '{"b":1}' with 0 in SQLite, not in Amphora); an argument that reads a table to its right or is given in WHERE; a virtual table that an extension writes.
- The sweeps hold five settings. Merge joins only and nested loops only were asked of the hand cases alone (0 differ). Statistics hide most of DT's 36: ANALYZE moves the order.
- Not run: macOS, the whole workspace's tests, `planner`, the corpus subset, a benchmark. What a subquery no longer put off costs was not measured: wrong rows came of it.
- Lost time: a probe over 300-row tables in a debug build held a run 5 minutes and answered one class; kiln's server gave no answer four times and lost two runs (r2060, r2075); a first run's script came in a mistyped base64.

## 2. Reproduced, smallest (SQLite 3.54.0 / Amphora main 20cffc15)

| finding | statement | SQLite | main | fixed |
|---|---|---|---|---|
| DT | m(k, v): (2, 1); q(k, v): (1, 2), (3, 2); w(k): (3): `SELECT a0.k, a1.v FROM m AS a0 LEFT JOIN m AS a1 ON a0.v = a1.v WHERE EXISTS (SELECT 1 FROM q AS s1 WHERE EXISTS (SELECT 1 FROM w AS s2 WHERE s2.k = s1.k) AND s1.v = a0.k) AND NOT EXISTS (SELECT 1 FROM m AS s9 WHERE s9.v = a1.k)` | `2,1` | no row (default, parallel, hash joins only) | SQLite's |
| DT | the finding's own (p, w, n, e of five rows and less) | 5 rows | 4 (hash joins only, merge joins only) | SQLite's |
| DS | `SELECT rowid FROM json_tree('[7]')` | `0; 1` | `NULL; 1` (all) | SQLite's |
| DS | a(k): 1, 2: `a AS x RIGHT JOIN json_tree('[7,1]') AS je ON 1` | 6 rows | 7, the root again unmatched (all) | SQLite's |
| DS | `SELECT x.k, je.rowid FROM a AS x LEFT JOIN json_each('[7,1]') AS je ON je.value = x.k` | `1,1; 2,NULL` | `1,2; 2,4` (all) | SQLite's |
| DS | `SELECT rowid, atom FROM json_tree('[[1,2],3]')` | 0, 1, 2, 3, 4 | NULL, 1, 2, 3, 5 (all) | SQLite's |
| DS | a(k, v) with a NULL k: `a AS t0 FULL JOIN generate_series(0, t0.k) AS t1 ON 1` | 11 rows | never ends (default, parallel, hash joins only) | SQLite's |
| DS | `a AS t0 JOIN b AS t1 ON t1.k = t0.k RIGHT JOIN generate_series(-2, 4, 2) AS t2 ON t2.rowid <= t1.k WHERE t0.k = t2.rowid` | 2 rows | 0 | SQLite's |

## 3. Traced: the first wrong place of each

- **DT.** Main's bytecode (the finding's own statement): s1's loop is `66 Rewind 6 .. 104 Next 6`, the grouped left join's `71 SeekGE 0 .. 99 Next 0` inside it, and the nested NOT EXISTS, s1's condition, is called and tested in the inner
  one: `94 Gosub 26`, `95 If r[2] -> 99`. It fails for s1's first row, the left join's loop ends, and control falls to `103 Goto 106; semi-join: early out after first match`, past `104 Next 6`: s1's other rows are not asked.
  The place: `count_subquery_calls_for_plan` (optimizer/join.rs) puts a correlated EXISTS off to a later loop where the plan's rows are fewer than its calls (`eval_after_table`), and skipped a semi or anti join only as the loop to put it in. The subquery reads
  s1, a semi join's table, and the loop it went to is after that join's. EXPLAIN QUERY PLAN, default: `SCAN a0, SEARCH a1 LEFT-JOIN, SEARCH s1, SEARCH exists_subquery LEFT-JOIN, SEARCH s2`: no hash join.
- **DS, 1 and 2.** `op_row_id_read` (vdbe/execute.rs): `(rowid != 0).then_some(rowid)` of `VirtualTableCursor::rowid`, which asked the module whether or not the cursor stood on an outer join's NULL row (its column() asked null_flag first, its rowid() did not).
  `emit_right_join_key` takes that rowid: the key of json_tree's root was NULL, and the unmatched rows' `Found` looked for it through a Bloom filter that drops NULL keys.
- **DS, 3.** `JsonEachCursor::next` (json/vtab.rs) counted `rowid` at each step, `filter` steps once before json_each's first row, and a container's end is a step. SQLite's `iRowid` is the row's number. A RIGHT or FULL JOIN keeps its matched rows by it, across documents.
- **DS, 4.** `a FULL JOIN generate_series(0, a.k)`: `9 VFilter 2` with a NULL argument gives no row, `24 NullRow 2`, `25 Goto 15`, and the NULL row's body falls to `21 VNext 2 -> 10`. `VirtualTableCursor::next` cleared null_flag and stepped the scan
  that VFilter had not begun: generate_series' cursor as it is opened has step 0 and never ends. SQLite's OP_VNext breaks on nullRow (vdbe.c).
- **DS, 5.** The plan materializes a hash join's build input from generate_series and a (`3 VOpen .. 37 VNext`), and `53 RowId 3` reads generate_series' ended cursor: `collect_materialized_payload_columns` (emitter/select.rs) kept the rowid of a B-tree table alone.

## 4. Fixed (49caf808)

- DT: a subquery is not put off past a semi or anti join whose table a term holding it reads (`semi_or_anti_join_of_terms`); docs/design/executor.md, decision 6, says so.
- DS: `VirtualTableCursor::rowid` is an Option, None on the NULL row; `next` ends on it; the JSON tables count rows apart from ids; the payload takes a virtual table's rowid.

## 5. Held

`bench/sqlite_joined_rows.py` (seed C24; its head names every exception), `tests/fixtures/joined_rows.tsv`, `tests/joined_rows.rs`: 2,900 queries SQLite answered, 14,500 answers.

- DS: 1,400 queries in two databases (with and without indexes): json_each (174), json_tree (344), generate_series (737) and pragma_table_info, index_info and index_list (644), 399 LEFT, 389 RIGHT and 402 FULL JOINs of up to three tables, 90 correlated
  arguments, rowid, oid and _rowid_ read in select lists, ON, WHERE, IN, EXISTS, scalar subqueries, aggregates, GROUP BY, DISTINCT, windows, CTEs, derived tables and UNION.
- DT: 1,500 queries in 30 databases (seven tables, empty or of 3 to 40 rows, NULLs and duplicates, half indexed): an EXISTS, NOT EXISTS (1,148), IN (866) or NOT IN (560) holding one that reads its table, under OR (708), CASE (197) and =, beside LEFT JOINs (885),
  a second subquery, an aggregate's value (260).

| `joined_rows`, answers that differ from SQLite's | default | parallel | statistics | both | hash joins only |
|---|---:|---:|---:|---:|---:|
| DS, main's engine (20cffc15), of 1,400 (and 3, 3, 2, 2, 3 that never end) | 870 | 870 | 871 | 871 | 870 |
| DT, of 1,500 | 36 | 34 | 11 | 12 | 38 |
| **the fixed engine, each class** | **0** | **0** | **0** | **0** | **0** |

Three more draws of the same size (seeds S1, S2, S3: 43,500 answers, r2092): 0 differ, 0 never end.

## 6. Checks

Linux, adacachy2. Red (r2105): the tests on main 3a8a80a0's six engine files (`json/vtab.rs`, `emitter/select.rs`, `optimizer/join.rs`, `optimizer/mod.rs`, `vdbe/execute.rs`, `vtab.rs`): `a_virtual_tables_rowid_is_its_rows_number_and_null_for_no_row`,
`a_semi_joins_condition_is_read_in_its_own_loop` and `subqueries_nested_in_joined_subqueries_answer_as_sqlite` (125 of 7,500 answers) failed; `virtual_tables_rowids_join_as_sqlites_do` was not run there, since three of its queries never end (its counts above are a probe's, query by query).
Green, on 24992ce6, the tree landed (run as b9f6299b: that tree, the probe and its cases, r2083): clippy `-p amphora-engine -p amphora-engine-parser --all-targets --locked -D warnings` 0 ("Checking amphora-engine", 43 s); check-production 0 ("engine-ratchet: 10568 left .. as the ratchet holds", disallowed_methods 40
to 39); `joined_rows` 2 passed (14,500 answers, 43 s), `columns_read` 5, `oracle_findings` 102, `join_methods` 4 (1 ignored), `adaptive_joins` 23, `outer_join_subqueries` 3; `cargo fmt --all --check` 0 (adacachy).
After the landing (r2092, the same engine): `unnesting` 2, `in_subquery` 4, `on_terms` 5, `windows_and_triggers` 4, `cte_columns` 11, `cte_scopes` 1, `hash_semi_join` 1, `nested_loops` 7, `plan_cache` 7, `row_values` 4, `dml_rows` 19, each passed. With DT's smallest form added to its hand test (cca6b524, r2105): `oracle_findings` 102, `joined_rows` 2.
The engine's own unit tests named vtab, series or json (`--lib`, r2105): 196 passed.

## Reproduce

```sh
python3 bench/sqlite_joined_rows.py > crates/engine/tests/fixtures/joined_rows.tsv
cargo test -p amphora-engine --features simulator --test joined_rows --test oracle_findings
```
