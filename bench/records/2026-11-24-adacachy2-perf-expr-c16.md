# Lane C16 (expr), 2026-11-24 09:42:02Z to 10:42:02Z: a recursive CTE's columns are typed by every arm, and its own arms read them so (the oracles' finding CU)

Machine: adacachy2 (runner 00aad8da, Linux, 32 cores, 0 to 21 of them idle all hour). Every build debug, `nice -n 10`, under `timeout`. Base main 70f6712b.
SQLite 3.54.0, check-in be8d059e9a49 (adacachy's library, through Python and the shell; the oracles' own build adds only SQLITE_LIKE_DOESNT_MATCH_BLOBS, no LIKE is here).
Runs named (r...) are in kiln's output log.

## 1. Losses first

- **CZ, DE and CX are not touched.** None of them has CU's first wrong place. What I saw of CZ and CX is in section 6; DE was not looked at.
- **Where SQLite's own answer to a mixed compound depends on how it reads it, Amphora is not held to it.** 261 of the 9,643 queries drawn are left out, each by a rule
  in the generator's head: 3.3.1's mixed affinities where SQLite pushes the WHERE into the arms, or where one arm alone is REAL (SQLite then makes the column's integers REALs as it
  reads them, `typeof(a)` real for integer; Amphora leaves them integers). On the fixed engine 23 of those 261 differ from SQLite's answer (r1652, default): 10 a REAL arm's, 3 a pushed WHERE's, 10 a mixed arm made of a scalar subquery. I did not look for more.
- **The recursive arms are planned twice where the arms make a column another than the initial query does** (an anchor `1` beside an arm `edge.dst`). Nested CTEs' plannings add up, they don't multiply
  (24 levels in 0.8 s of a debug build). The cost of the second planning is not measured: no benchmark was due, and none was run.
- The sweep compares rows as a set (the two `ordered` tails as a list); which of a group's equal values comes back, and a REAL's text, are not compared.
- The `planner` suite is 13 of 14; `a_whole_table_group_by_scans_and_sorts_where_lookups_would_read_pages` fails, as on main (lane C9's record, the same one).
- Lost time: four runs refused for room (4, 5, 2 and 0 to 3 of 32 cores idle), one taken by a server that did not answer (r1712), one stopped because a test name needs a feature (r1714).

## 2. Reproduced, smallest

```sql
CREATE TABLE edge(src TEXT, dst TEXT); INSERT INTO edge VALUES ('1', '2'), ('2', '3');
WITH RECURSIVE r(n) AS (SELECT 1 UNION ALL SELECT r.n + 1 FROM edge JOIN r ON edge.src = r.n) SELECT n FROM r;
```

| setting | SQLite 3.54.0 | Amphora main | fixed |
|---|---|---|---|
| default, parallel, statistics, both | 1, 2, 3 | 1 | 1, 2, 3 |

The same at the four settings for the finding's own form (`r(n, d) .. WHERE r.d < 5`): 1,0 2,1 3,2 against 1,0.

## 3. Traced

- The plan and the bytecode are the same on main and fixed (`SCAN edge`, `SCAN r`; instruction 18 `Ne r[11] r[12]`, collation Binary), but for the comparison's affinity, which EXPLAIN does not print.
  Traced by an eprintln in `comparison_affinity` (not for main) over this statement, on both engines:

| | left, `edge.src` | right, `r.n` | affinity compared by |
|---|---|---|---|
| main | Text | **Blob** | Blob: neither side converted |
| fixed | Text | None | Text: 1 becomes '1' |

- SQLite's own bytecode for the same shape (a column of `r` compared with a constant) is `Ne .. BINARY-8`, p5 82: TEXT affinity (0x42) where the anchor is a TEXT column, so a recursive arm does read a typed column.
- **First wrong place:** `JoinedTable::new_recursive_cte_input` (translate/plan.rs), Turso's, put BLOB on every column of the table the recursive arms read the CTE by ("the recursive self-reference
  reads SQLite's queue table, whose columns have no declared type"). BLOB is an affinity, not none (datatype3.html 4.2): against TEXT no side is converted.
- The second half of the same place: `query_output_columns` gave the query around the CTE the initial query's affinity alone (and, of a compound initial query, its last arm's column, not its first's).
- **What SQLite does:** one table serves the recursive arms and the outer query; its columns are typed once, from every arm left to right, each arm's read of the CTE's own column seeing what is typed so far
  (`resolveFromTermToCte`, `sqlite3SubqueryColumnTypes`, select.c; checked on 3.54.0 with eleven statements, in tests/cte_columns.rs).
- **What the documents say:** datatype3.html 3.2: a column has its affinity, a CAST its type's, "Otherwise, an expression has no affinity"; 3.3: a subquery's columns "are really the expressions in the result set"; 3.3.1: a compound's column has
  "the affinity of the corresponding result column for one of the individual SELECT statements", which one is indeterminate; 4.2, "If one operand has TEXT affinity and the other has no affinity, then TEXT affinity is applied to the other operand."

## 4. Fixed (a9a11b11)

- `CompoundColumnAffinity` (plan.rs) is the one reading of a compound's column, an arm at a time; `recursive_cte_columns` types a recursive CTE's columns by it, in order.
  `RecursiveCtePlan.columns` is what the outer query reads, and the recursive arms' input table has the same columns.
- The arms are planned by the initial query's types, and again only where the arms make a column another. While a recursive CTE's arms are planned for the first time, a nested one whose types come out otherwise tells
  it to plan again (`ProgramBuilder::typing_recursive_arms`), so nested CTEs are planned a number of times that grows with depth, not 2^depth.
- The removed indexing and arithmetic lowered the ratchet: indexing_slicing 3054 to 3048, arithmetic_side_effects 3281 to 3277.
- e89b0e9a, found by the sweep: a row value's comparison took the right column's NOCASE where its left column declares no collation (`(w.a, w.id) = (o.tn, o.id)`: 2 rows for SQLite's 1); datatype3.html 7.1 gives the left column's BINARY.

## 5. Held

`tests/cte_columns.rs`, `bench/sqlite_cte_columns.py` (the generator, SQLite's answers), `tests/fixtures/cte_columns.tsv`:

- 9,382 queries, 40 sources (a column of each of INTEGER, REAL, TEXT, NUMERIC, BLOB, none, NOCASE and RTRIM and their COLLATEs, 6 CASTs, literals, expressions, scalar subqueries), 61 shapes (recursive CTE arms and outers, CTEs plain, MATERIALIZED and not,
  a CTE of a CTE, derived tables, UNION, UNION ALL, EXCEPT, INTERSECT, mixed arms), 303 tails (=, <, >=, IS, BETWEEN, IN a list, IN a subquery either way, row values, join keys with and without an index, LEFT JOIN, WHERE, EXISTS, ORDER BY, GROUP BY, HAVING, DISTINCT,
  windows), over a column of every type, rows of NULL, integers, REALs, texts that are numbers and not, and BLOBs. Each at default, parallel, statistics and both: 37,528 runs.
- Two tests by hand: the statements of section 2 and their neighbours (18 at four settings), and 24 nested CTEs.

| `cte_columns`, wrong of 5,656 recursive / 3,726 other queries | default | parallel | statistics | both |
|---|---:|---:|---:|---:|
| main's engine (70f6712b; the 392 of a scalar subquery not counted) | 483 / 1 | 483 / 1 | 482 / 1 | 482 / 1 |
| with a9a11b11 alone (the same) | 0 / 1 | 0 / 1 | 0 / 1 | 0 / 1 |
| with e89b0e9a too, as landed | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 |

The first two rows are r1652's logs of the sweep's first draw (9,643 queries, the exceptions not yet named: 585, 585, 584 and 584 recursive wrong on main, 77 with the fix; 52 others on both), read
for the queries kept. On main by shape: the recursive arm's own reads 400 (`step_where` 68, `step_scan` 66, `step_left` 60, `step_case` 58, `step_union` 52, `step_exists` 49, `step` 47) and mixed arms 83, 46 of them read around the CTE; the one other is the row value's.

The documented exceptions, each in the generator's head: a scalar subquery's column has no affinity (datatype3.html 3.2; SQLite gives it its column's; the document is held to: 392 queries); 3.3.1's mixed arms (above); CY; CK.

## 6. Seen on the way, left

- CZ: both engines give the merged column the left column's affinity (TEXT over INTEGER: `k = 2` finds the right table's 2 on both). The finding's form is a TEXT left and a REAL right: 0.0 compared with 0 by TEXT.
  SQLite's OP_Eq converts under TEXT affinity only where one side is a text, so 0 = 0.0 (1 row); Amphora's rows say it converts both ('0' and '0.0', no row). From the finding and SQLite's source as I know it; Amphora's opcode not read.
- CX, run on the fixed build: a trigger's `new.name = 'admin'`, NOCASE, 'ADMIN': the WHEN never fires, the body's comparison is 0, `new.name IN ('admin')` 0, `new.r = 'x'` (RTRIM, 'x  ') 0; SQLite 1, 1, 1, 1. `new.c` becomes an `Expr::Variable`
  that carries a type name and no collation. Its affinity is right as it is: SQLite's `new.n = '1'` (INTEGER n) is 0 too.
- DE: not looked at.
- EXPLAIN prints p5 0 for every comparison, so the affinity and NULL handling a comparison is compiled with can't be read from it.
- A recursive CTE's queue (`Plan::select_result_columns` of a RecursiveCte) still reports the initial query's last arm for callers other than its columns.

## 7. Checks

Linux, adacachy2. On the merged tree 1bf89d26 (main b549489e in it, lane C13's changes to the same functions): `cargo fmt --all --check` 0; clippy `-p amphora-engine --all-targets --locked -D warnings` 0 ("Checking amphora-engine", 42 s);
check-production 0 ("engine-ratchet: 10606 left .. as the ratchet holds", 10,616 before); `cte_columns` 10 passed (37,528 runs, 94 s at 2 threads), `oracle_findings` 91, `on_terms` 5, `outer_join_subqueries` 2, `in_subquery` 4 (r1770, r1774).
On 8565db4f, before lanes C13's, B26's and A28's landings (`simulator` feature, r1725): `lib translate::` 299 passed, 22 ignored; `unnesting` 2, `join_methods` 4 (1 ignored), `adaptive_joins` 23, `row_value_seek` 5, `nested_loops` 7, `planner` 13 of 14 (main's own), clippy 0.
Not run: macOS (r1653 and r1772 never found room), the whole workspace's tests, the corpus subset, a benchmark.

## Reproduce

```sh
python3 bench/sqlite_cte_columns.py > crates/engine/tests/fixtures/cte_columns.tsv
python3 bench/sqlite_cte_columns.py --sql step one eq_t     # one query, and SQLite's rows
cargo test -p amphora-engine --test cte_columns -- --nocapture
```
