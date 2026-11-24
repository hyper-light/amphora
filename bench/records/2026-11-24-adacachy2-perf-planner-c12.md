# Lane C12 (planner): oracle finding CN, a hash join's null-extended rows and WHERE terms on subqueries (2026-11-24)

Hour: 09:29:01Z to 10:29:01Z. Machine: adacachy2 (Linux x86_64, 32 cores, shared: 1-minute load 16 to 149), debug builds (`--features
simulator`); SQLite 3.54.0 is the reference (adacachy's and adacachy2's system library). No timing is in this record.

Losses first.

- **Speed is not measured** (Rule 0: correct first), and three things change it:
  - A hash join over a materialized input of several tables is no longer planned where a correlated subquery reads one of those tables; the join
    takes the next plan. Each such plan read the subquery's outer columns from a cursor left at its end.
  - In a query with aggregates, a correlated subquery on an earlier loop's table, read beside a hash LEFT JOIN's column, runs for each
    row there; before, once for each row of the earlier loop.
  - A matched row of a hash LEFT JOIN that fails a WHERE term costs a `Gosub` and a `Return` more.
- **DJ, new, not fixed, not traced.** A row value's `NOT IN` answers otherwise than SQLite where a value is NULL, by every join method and with
  no hash join in the plan: `SELECT a0.k, a1.k FROM g AS a0 FULL JOIN e AS a1 ON a0.k = a1.k WHERE (a1.k, a0.k) NOT IN (SELECT s1.k, s1.v FROM f AS s1
  WHERE s1.k <= s1.k)` (e empty): SQLite 3 rows, Amphora 0; 14 of the 16 queries whose rows stayed wrong after the first fix, on main before. The
  sweep draws no row value under a NOT and says so.
- CP shows in C10's generator still (5 of 15,000 answers, seed y1: `a1.s IN (SELECT value ... FROM json_each(...))`), as CP's own.
- The sweep's queries are small tables (0 to 8 rows): a spilled (grace) hash join's unmatched rows take the same subroutine, but no drawn query spills.

## Reproduced (done when 1)

CN's query on main 7173ca16, at 6 settings; its smallest form is an IN whose list is correlated with a table of the build's input:

```sql
CREATE TABLE p(id INTEGER PRIMARY KEY, k INT, v INT);  INSERT INTO p VALUES (1, 1, 2), (2, 2, 3), (3, NULL, 1), (4, 4, NULL), (5, 1, 0);
CREATE TABLE q(id INTEGER PRIMARY KEY, k INT, v INT);  INSERT INTO q VALUES (1, 2, 2), (2, 2, 0), (3, 5, 1), (4, NULL, 4), (5, 1, 1);
CREATE TABLE n(k INT, v INT);  INSERT INTO n VALUES (1, 2), (NULL, 1), (2, 2), (3, 0), (1, NULL);
CREATE TABLE e(k INT, v INT);  ANALYZE;
SELECT a0.k, a1.k, a2.k FROM p AS a0 JOIN q AS a1 ON a0.k = a1.k LEFT JOIN e AS a2 ON a1.k = a2.v WHERE a2.k IN (SELECT n.k FROM n WHERE n.v = a0.k);
-- SQLite 3.54.0: no row.  Amphora: 4 (1,1,NULL twice; 2,2,NULL twice).  default and parallel: no row; with statistics, with and without parallel: 4.
```

The class is wider than null-extended rows. At defaults on main: `SELECT a1.k, a3.v FROM n AS a0 JOIN p AS a1 ON a0.k = a1.k LEFT JOIN q AS a2 ON a1.k =
a2.k LEFT JOIN g AS a3 ON a2.k = a3.v WHERE coalesce((SELECT count(s1.k) FROM n AS s1 WHERE s1.k IS a3.v), a1.k) = a0.k`: SQLite 0 rows, Amphora 12. With
statistics, an inner join loses rows: `... JOIN f AS a2 ON a1.k = a2.v WHERE (SELECT a0.k) IS a2.v`: SQLite 4, Amphora 0.

## Traced (done when 2): four places, each the first wrong one for its queries

1. **The planner took a0's loop from the subquery that reads it.** Plan of CN with statistics: `MATERIALIZE hash build input for q AS a1` (p joined to q), `HASH JOIN
   e AS a2`, and p has no loop after it. The subquery's outer column is `59 Column 1,1` , p's cursor, which the input's loop `51 Next 1` left at its end. The rule
   that refuses such a join (`should_not_use_hash_join`) looked at the build and the probe table alone, not at the other tables the input joins.
2. **The unmatched rows' path had its own copy of the WHERE terms, and dropped one it thought unreadable**: `105 HashScanUnmatched`, `106 Gosub 55,95`, the row's
   output. A subquery coded in the probe's loop was read from its register there, another row's. With 2 alone fixed CN's query is right and 7 of its 290 forms are
   wrong, rows lost among them (`a2.k IS NOT NULL OR EXISTS (... n.v = a0.k)`: SQLite 4, 0): 1 is why.
3. **A term with a correlated subquery was a key of the hash join.** `12 Copy 1 -> 7`, `17 HashBuild ... 2 keys` before the subquery at 64 is ever run:
   `one_table_in_expr` counts a side's columns and not what a subquery in it reads.
4. **An uncorrelated subquery was coded twice, for the materialized input and for the query**, its hash join built by the first copy, which nothing called:
   `55 HashBuild 10` in 42..86 (no Gosub to it), `177 HashProbe 10` in 165..199, with no build. EXISTS was false and a sum NULL, on every row.
   In the same plans `build_prefilter_where_terms` met a term whose subquery is not the input's: "Parse error: subquery not found" (196 answers on main).

## The fix

- `join_lhs_and_rhs`: no materialized input of a table a correlated subquery reads; `build_where_term_info`: no hash key holds a correlated subquery.
- The matched rows and the unmatched ones enter one subroutine, which begins before the level's WHERE terms (`open::begin_hash_join_rows`), so the terms and their
  subqueries are coded once and a term can't be left out. A hash anti join's own terms (those that read its probe table) come before it. Gone: the
  second copy (`emit_unmatched_row_conditions_and_loop`, `condition_operands_are_available`), and the rule that ran a subquery early for its sake.
- A materialized input's plan holds the uncorrelated subqueries alone, and the query's plan takes those as coded: one subroutine, run once, for whichever reads it first.

## Held by tests (done when 3)

- `joins_filtered_by_subqueries_answer_as_sqlite_by_every_join_method` (`tests/outer_join_subqueries.rs`): 500 queries over two databases (tables with
  indexes and without; `bench/sqlite_join_where_subqueries.py` draws them and asks SQLite): joins of 2 to 4 tables, inner, LEFT, RIGHT, FULL; scalar, EXISTS,
  IN, NOT IN and row value IN, correlated with none to two of the tables or with a query around; in WHERE, ON, the select list, GROUP BY, ORDER BY and under
  aggregates. 11 settings (default; parallel; statistics; both; hash joins; hash joins with statistics, parallel too; merge joins, with statistics too;
  nested loops; those with both): 5,500 answers.
- `a_hash_joins_rows_are_tested_by_where_terms_on_correlated_subqueries` (`tests/oracle_findings.rs`): CN's query, the forms above, 12 queries at 7 settings.

| run | tree | answers | differ | errors | wrong rows |
|---|---|---:|---:|---:|---:|
| drawn, seeds w1 to w4 (row value NOT IN drawn then) | main 7173ca16 (5c55f2e6: the tests) | 132,000 | 560 | 196 | 364 |
| the same | 57f266cd: causes 1 and 2 fixed | 132,000 | 413 | 275 | 138 (2 queries but for DJ's) |
| drawn, seeds x1 to x3 | 7779647f: cause 3 left | 99,000 | 7 | 0 | 7 (one query: cause 3) |
| drawn, seeds x2 and x5 | e9ffdc42, the fix | 66,000 | 0 | 0 | 0 |
| C10's generator, seed y1, 5 settings | 7779647f | 15,000 | 5 | 0 | 5 (CP, by its form) |
| the landed sweep (its first drawing) | main 7173ca16 (5c55f2e6: the tests) | 5,500 | 49 | not split | not split |
| the landed sweep, as landed | e9ffdc42, the fix | 5,500 | 0 | 0 | 0 |

No panic in any of these runs, and every table held its rows after each query. CN's 29 forms at 10 settings on d671229e, cause 2 alone fixed: 283 of 290 right.

## Checks (Linux, adacachy2)

- On the merged tree (869a4f62, main b549489e; the landing merge dea76613 has its code): `cargo fmt --all --check` 0; clippy `-p amphora-engine --all-targets`
  0 ("Checking amphora-engine"); outer_join_subqueries 3 passed (13,500 answers), oracle_findings 92; check-production @PROD@; @REST@.
- On e9ffdc42 (the fix before lane C13's landing, the tree of 7e9e52b1): clippy 0, outer_join_subqueries 2, oracle_findings 87, C6's four small shards 4
  passed (26,880 runs), lib `optimizer::` 185 of 208 passed and none failed when I stopped it for the merged tree's run (the machine's load was 149).
- On 7779647f (causes 1, 2 and 4): in_subquery 4, join_methods 4 (1 ignored), adaptive_joins 23, unnesting 2 (simulator), hash_semi_join 1; planner 13 of 14
  (`planner.rs:182`, main's own then).
- Not run: the new test of oracle_findings on main without the fix (the sweep's red run is in the table; CN's forms were run by hand on main: 3 of 29 wrong
  with statistics, parallel or not; 1 of 12 in the second set).

## Reproduce

```sh
python3 bench/sqlite_join_where_subqueries.py 250 CN > crates/engine/tests/fixtures/join_where_subqueries.tsv   # SQLite's answers
cargo test -p amphora-engine --features simulator --test oracle_findings --test outer_join_subqueries
```
