# Lane C18 (planner): CTE scopes: oracle finding DH landed, DI fixed, and the uses of one CTE that shared a plan (2026-11-24)

Hour: 10:23:49Z to 11:23:49Z. Machine: adacachy2 (Linux x86_64, 32 cores, shared: 1-minute load 15 to 57), debug builds (`--features
simulator`), lane C13's target folder; SQLite 3.54.0 is the reference (adacachy's and adacachy2's system library). No timing is in this record.

Losses first.

- **Four defects found on the way are open**, none of them a CTE's scope but the last:
  - DL: a FROM clause's subquery inside a HAVING's subquery loses the grouped row it reads (no CTE in it; SQLite 2 rows, Amphora 0). The sweep's
    HAVING reads nothing of the grouped row, so it does not draw it.
  - DM: a compound's LIMIT that is a subquery is refused, "datatype mismatch".
  - A cycle's error names another CTE of the cycle than SQLite's does where more than one is met: the test compares "circular reference" but the name.
  - DN: in a CTE g, a UNION arm's `FROM g` is SQLite's recursive table even where g's own body has a WITH that defines g; Amphora reads the
    inner one, and answers a row where SQLite answers none (one statement of the first wide seeds; the draw leaves it out).
- **The sweep ends its process on main** ("has overflowed its stack", rc 134): one of its statements recurses without end in main's planner, so the
  sweep's red count on main is not known, nor which statement. The red of DH, DI and the uses is `oracle_findings`' and the reductions' (below).
- `planner` is 13 of 14 in the last run (`a_whole_table_group_by_scans_and_sorts_where_lookups_would_read_pages`, the cost of a GROUP BY that lanes C4, C9, C10
  and C13 recorded of main); it was 14 of 14 on 88e955aa. Run alone, it fails on
  main's engine from before this landing (837071af) 3 times of 3, and passes on this one's 2 times of 6 (6692bdf5, 200e6d31): `planner.rs:182`, 800 pages; the statement holds no CTE. It is main's.
- Of the lib suites, `cte` 56, `subquery` 9 and `translate::` 113 (without `optimizer`) passed on the landed tree (200e6d31, after the merge); `optimizer::` and C6's shards were not run on it.
- A statement with an unknown column's qualifier says "no such table: d" for SQLite's "no such column: d.k" (main's, not touched): no draw holds it.
- Each use of a sibling CTE beyond the first, in a body's nested query, is planned anew: the work of planning doubles for each level of a chain of
  CTEs that each read the one before in two nested queries. SQLite's does for every second read (and refuses the statement at 14 levels, "nested too deep"; 1 s at 12); not measured here.

## Done when 1: DH on the tree with lane C16's changes

`108d3dbb` (lane C13's fix on fedc9015, merged with C16's landing) merged into main 4b5c78b4 as 65bd224f, and run there: clippy 0, oracle_findings 92,
outer_join_subqueries 2 (8,000 answers), cte_columns 10. DH's query 1 row (SQLite's), the DG shape 2, 2 (SQLite's).
DH's fix picked the names a body may read by the distance of entries (`cte_body_refs`). It is replaced: a probe of 97 statements found
wrong rows and panics under it, the same cause in each (below).

## Done when 2: DI, and the suspected shared plan, traced

- **DI**: `collect_from_clause_table_refs` collected the names in every body of a WITH clause inside, read or not, and the names those CTEs define. It now resolves as
  SQLite does: the names the query reads, and the body of a CTE when something reads it. `referenced_cte_indices` is computed from it, so two
  siblings of which one only names the other in an unread or an inner CTE were "circular reference" (SQLite answers), and are not now.
- **The suspected plan is real**, shown by a query on 65bd224f (and on main): `WITH c AS (SELECT k FROM q) SELECT * FROM (WITH a AS (SELECT k FROM c),
  b AS (SELECT k FROM c) SELECT a.k AS ak, b.k AS bk FROM a, b)`: SQLite 9 rows, Amphora 3; over a recursive c a panic, "cursor id 0 is None".
- **Cause.** An entry held a plan, or the syntax, and not the scope of a CTE: a use in a nested WITH's body took the plan (`base_outer_refs_for_cte_planning` kept the
  syntax back), so two uses had one set of cursors; and a body planned from syntax read its names from the reader's scope. The shared rows of a
  materialization are counted per query, so uses in different nested queries of one body shared nothing but the cursors.
- **Fix.** An entry holds its WITH clause's CTEs and the names around that clause (`CteScope`, `CteWith`), and a use is planned from there (`plan_cte`), in a FROM clause, a subquery, a
  derived table, a nested WITH, or a LIMIT alike. One use may take the plan an entry holds; one beside it in the same FROM clause shares it (they are materialized once);
  each other has a plan of its own. A view's body is planned with none of the caller's names. An INSERT's WITH stands around its SELECT's.
- **Found by the same probe and fixed**: a view's body read the caller's CTEs (0 rows for 1); a derived table under a WITH that takes an outer CTE's name read the outer one; a correlated CTE read twice by another
  (3 for 9); a CTE's body could not read a table around it of the CTE's own name; a LIMIT's subquery saw no CTE, or the table of its name;
  INSERT's WITH and its SELECT's were one clause (a duplicate name was an error); the left side of IN (SELECT) was not searched for CTEs; a column list too short was
  not refused for a CTE read by another; `RecursiveRefCounter` took a nested CTE's later sibling for the recursive table ("circular reference"); the stack of CTEs being
  defined lost its order under an inner WITH, so a later use was "circular reference".

## Done when 3: the sweep

`ctes_of_with_clauses_inside_one_another_answer_as_sqlite` (`tests/cte_scopes.rs`, fixture `cte_scopes.tsv`, `bench/sqlite_cte_scopes.py`): 1,200 statements, each with
SQLite's rows or its error, in three databases (reads without and with indexes, writes), at default, parallel, statistics and both: 4,800 answers. WITH clauses to three levels,
names reused between them and from tables and views, unread and recursive CTEs, forward references, CTEs read twice, in views, derived tables, ON, HAVING,
ORDER BY, LIMIT, windows, a table function's argument, compounds, INSERT SELECT and VALUES, UPDATE (with FROM) and DELETE; correlated bodies.

Red, on main's engine (0c0a3b6c) with these tests (diag/planner-c18-red, ba8be6ef): `oracle_findings` 92 passed, 3 failed (DH's: 2 for 1; DI's: "no such table"; the uses': 3 for 9); the sweep
ends the process; of 80 reductions of one panic (a recursive CTE that a body reads only in nested queries), 16 panic. On 65bd224f (DH alone) 11 of 52
statements of the first probe were DI's, 4 the shared plan's (one a panic) and 1 the view's.

| run | tree | answers | differ | what |
|---|---|---:|---:|---|
| the sweep's first draw | f0f2bd82 | 4,800 | 400 | 100 statements: INSERT's two WITHs, LIMIT, false circles, a column list, several errors at once |
| the same, one fault a statement | a8e7ae85 | 4,800 | 0 | |
| drawn, seeds s1 to s4 | 5641ce64 | 14,400 | 8 | DN, and a message SQLite overwrote |
| the landed fixture | 17af1f54 | 4,800 | 4 | one statement, a panic (the shape of the 80 reductions) |
| drawn, seeds s1 to s6 | 17af1f54 | 21,600 | 12 | false circles: the order of the CTEs being defined |
| the landed fixture | 6692bdf5: the landed engine | 4,800 | 0 | |
| drawn, seeds s1 to s6 | 6692bdf5 | 21,600 | 0 | |
| lane C13's sweep, drawn, seeds w1, w2, w3, w4, w7, w8 | 6692bdf5 | 45,000 | 0 | w1 holds DH and DI: 10 before |
| reductions: p1, p4, p5, p6 (161 statements, default) | 6692bdf5 | 161 | 0 | p2 (45): DM, and two texts of "no such column" |

## Checks (Linux, adacachy2)

- 6692bdf5 (the landed engine and tests; main 837071af, lane C17's landing, merged): fmt 0; clippy `-p amphora-engine --all-targets` 0 ("Checking
  amphora-engine", 58 s); oracle_findings 95, cte_scopes 1 (4,800 answers), outer_join_subqueries 3, cte_columns 11, on_terms 5, adaptive_joins 23, in_subquery 4, join_methods 4 (1
  ignored), hash_semi_join 1, unnesting 2 (simulator), top_n 3, hash_group_by 5, upsert_or_clause 2, planner 13 of 14 (above).
- eb28e119 (6692bdf5 and the ratchet): check-production 0 (10,591, as the ratchet holds: expect_used 856 to 855, indexing_slicing 3,046 to 3,042).

- 200e6d31 (main, after the merge): lib `cte` 56, lib `subquery` 9, lib `translate::` 113 without `optimizer`.

## Reproduce

```sh
python3 bench/sqlite_cte_scopes.py 400 C18 > crates/engine/tests/fixtures/cte_scopes.tsv   # SQLite's answers and errors
cargo test -p amphora-engine --features simulator --test oracle_findings --test cte_scopes --test outer_join_subqueries --test cte_columns
python3 bench/sqlite_cte_scopes.py 300 s1 > s1.tsv   # a wide seed: the same test, its fixture read from this file
```
