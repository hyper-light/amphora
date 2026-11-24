# Lane C10 (planner): oracle finding CH, an outer join's read of its unmatched rows ran copies of subqueries (2026-11-24)

Hour: 08:46:48Z to 09:46:48Z. Machine: adacachy2 (Linux x86_64, 32 cores, shared: 1-minute load 24 to 58), debug builds (`--features
simulator`); SQLite 3.54.0 is the reference (adacachy's and adacachy2's system library, 2026-10-09). No timing is in this record.

Losses first.

- **Five defects the sweep found on the way are on main still** (each in plain SQL, none CH's, none fixed here; findings.md):
  - CM: a RIGHT or FULL JOIN whose right table is WITHOUT ROWID (experimental) with a secondary index errors, with a subquery or
    without: `SELECT count(*) FROM e AS a0 FULL JOIN w AS a2 ON a0.k = a2.v`: SQLite 3, Amphora "secondary index lacks primary key locator".
  - CN: with statistics, a LEFT JOIN run as a hash join on a materialized build does not test a WHERE term on a correlated subquery for
    the null-extended rows: SQLite 0 rows, Amphora 2. Traced: bytecode goes from `HashScanUnmatched` to the row's output;
    `emit_unmatched_row_conditions_and_loop` (`main_loop/body.rs`) drops a term whose operands it finds unavailable.
  - CO: a CTE read by a subquery in an ON clause is not found, in any join: `WITH c AS (SELECT x FROM r) SELECT a.x, b.x FROM o AS a
    JOIN o AS b ON a.x IN (SELECT c.x FROM c)`: SQLite answers, Amphora "no such table: c".
  - CP: a table function's untyped column compares with a TEXT column as TEXT: `SELECT p.s, j.key FROM p, json_each('[5,6,7]') AS j
    WHERE p.s = j.key`, p.s = '1': SQLite 0 rows, Amphora 1.
  - CQ: a table function right of a FULL JOIN whose argument holds a correlated subquery reads the argument of the row before
    (`FROM a FULL JOIN json_each((SELECT t.j FROM t WHERE t.k = a.k AND EXISTS (...))) AS je ON je.value = 7`: SQLite `1,7; 2,NULL`,
    Amphora `1,NULL; 2,7`), on main before this lane too. That argument is the one place a subquery is still coded twice, so
    one that holds an uncorrelated subquery which hash joins still loses rows under a left table with no row (SQLite 2, Amphora 0).
- The drawn sweep draws no WITHOUT ROWID table on the right of a RIGHT or FULL JOIN (CM) and no CTE in an ON clause (CO), and says so.
- `crates/engine/tests/planner.rs:182` fails as on main before (lane C4's record: red since 18daab1e).
- A read of the unmatched rows no longer seeks by a term that reads a correlated subquery (it read by the copy's value); it reads the table and the row
  body decides. Not measured (Rule 0: correct first). An uncorrelated subquery's seek is kept, and it runs once where it ran twice.

## Reproduced (done when 1)

The smallest form, and the four findings' own (norec-d683f574f319ea5f, norec-81a40e3e4ac6bf23, norec-79280259aa4b7b64, diff-57b34fd6d54e9f91), on main f183dd75:

```sql
CREATE TABLE o(x INT);  INSERT INTO o VALUES (1), (2);
CREATE TABLE r(x INT);  INSERT INTO r VALUES (1), (5);
CREATE TABLE l(x INT, y INT);  INSERT INTO l VALUES (2, 5), (3, 7);
SELECT a.x, b.x FROM o AS a FULL JOIN o AS b ON NULL
WHERE (SELECT count(*) FROM r WHERE r.x IN (SELECT l.y FROM l WHERE b.x <= l.x));
-- SQLite 3.54.0: NULL,1 and NULL,2.   Amphora: a panic.
```

The panic's stack: `Option::expect` (btree.rs:3155) < `indexbtree_seek_unpacked` < `do_seek_unpacked` < `seek_unpacked` < `seek_internal` < `op_found`
< `normal_step`; then on the statement's reset `Pager::rollback` (pager.rs:7106, "dirty pages should be empty for read txn").

**It is worse than a panic.** `op_open_ephemeral` empties a cursor's B-tree when it finds the cursor open (`ClearExisting`), and the cursor was the outer
table's. The same statement over a WITHOUT ROWID table (whose cursor is an index's, so nothing panics) emptied the table: in a drawn
sweep on main every later query of it answered over no rows (3,346 of 20,000 answers, no panic). The "dirty pages" panic is that write, met in
a read transaction.

## Traced (done when 2)

- The plan (EXPLAIN QUERY PLAN, main): the loops' `CORRELATED SCALAR SUBQUERY 1` is `HASH JOIN l, SCAN r` (the IN unnested into a hash semi join); the
  unmatched rows' copy `CORRELATED SCALAR SUBQUERY 2` is `CORRELATED LIST SUBQUERY 3 SCAN l, SCAN r`: not the same plan.
- The program: `1 OpenRead 0 root=2` (o), ... `81 OpenEphemeral 0`, `89 IdxInsert 0`, `99 NotFound 0`.
- Why cursor 0. The parser gives each IN's list a cursor as it meets it, so the innermost IN's is 0.
  `plan_unmatched_right_rows` cloned the WHERE clause's subqueries while the table reads were planned, before
  `plan_correlated_subqueries` planned (and unnested) them, so the copy kept the IN. `optimize_plan` then released the cursors the optimized
  plan no longer held, and `subquery_resources`, which lists what a plan holds, read no table's `unmatched_right_rows_plan`. The outer table's first `resolve_cursors` took 0.
- What the first wrong place is: the copy. Counted as a holder alone (the first fix tried), the panic was gone and four drawn sweeps of 20,000 runs still
  had 6 panics ("Cannot insert into hash table in unexpected state", "cursor id 7 is None") and 4 wrong answers of the same cause, and the loss above
  in plain SQL: the copy's hash tables and Once blocks are the loops' by their tables' ids (a hash table is named by its build table), so the
  copy probed one that only the loops built, or built into one already probed.
- The copy narrowed the read only. The shared row body (the subroutine `Gosub`ed for each unmatched row) tests every WHERE term; SQLite's
  `sqlite3WhereRightJoinLoop` duplicates the WHERE clause for the same reason and recompiles its subqueries whole.

## The fix

- A term on a correlated subquery is left to the row body. An uncorrelated subquery is run once, where first read, for the loops and the read both: the
  read calls the loops' subroutine (`emit_lazy_subquery_call`).
- A list the read seeks by is stepped through by a cursor of its own (`OpenDup`, now for an ephemeral index too), because the row body looks rows up in it
  meanwhile.
- The copy is kept for a table function's arguments alone, which the read cannot leave out, and is now counted as held.

## Held by tests (done when 3 and 4)

- `a_right_or_full_join_over_nested_subqueries_returns_sqlites_rows` (`tests/oracle_findings.rs`): the four findings, their plain forms, the lost
  rows, and a list that is the read's seek, at default, parallel, statistics, both, hash joins only, nested loops only; every table is read after
  each query and holds what it held.
- `outer_joins_with_nested_subqueries_answer_as_sqlite` (`tests/outer_join_subqueries.rs`): 600 queries (2 databases, with and
  without indexes; `bench/sqlite_outer_join_subqueries.py` draws them and asks SQLite), 5 settings, 3,000 answers.
- No query found needs a refusal: SQLite answers each, and Amphora answers each now, so there is no error value to assert (done when 4).

| run | tree | answers | differ | panics | errors | wrong |
|---|---|---:|---:|---:|---:|---:|
| drawn, seed m2, 4 settings | main f183dd75 | 20,000 | 538 | 24 | 12 | 502 |
| drawn, seed m4, 4 settings | main f183dd75 | 20,000 | 3,358 | 0 | 12 | 3,346 |
| drawn, seeds m1 to m4 | the copy counted as held, not removed | 80,000 | 56 | 6 | 44 | 6 |
| drawn, seeds k1 and k2 (CTEs, json_each added) | ffd0ac6f, the fix's engine | 48,000 | 18 | 0 | 0 | 18 (16 CP, 2 CN) |
| the landed sweep, 5 settings | d8adcbe2 (tests and fix) | 3,000 | 0 | 0 | 0 | 0 |
| the landed sweep, 5 settings | 238d54b0 (the tests, main's engine) | 3,000 | 50 | 33 | 0 | 17 |

In the last row's run 22 of the 50 also left a table of the database empty (`n`, 5 rows, read after the query on the same connection: 0). Panics there: 17
"IdxInsert: not a BTreeIndex cursor", 5 "indexbtree_seek_unpacked: index_info required", 5 and 5 "cursor id 6 (7) is None", 1 "Cannot insert into hash table".

Red and green in the landed tests (Linux): red (238d54b0): the findings' test panicked at btree.rs:3155 ("indexbtree_seek_unpacked: index_info required") and on
its statement's reset at pager.rs:7106 ("dirty pages should be empty for read txn"), and the sweep is the last row; green (d8adcbe2): oracle_findings 85 passed,
the sweep 1 passed.

## Checks (Linux, adacachy2; d8adcbe2 unless said)

- clippy `-p amphora-engine --all-targets` 0 ("Checking amphora-engine"); `cargo fmt --all --check` 0; check-production 0 (10,617, as the ratchet holds).
- oracle_findings 85, outer_join_subqueries 1, in_subquery 4, join_methods 4 (1 ignored), adaptive_joins 23, unnesting 2 (simulator), hash_semi_join 1,
  lib `subquery` 9, planner 13 of 14 (182, main's own), lib `optimizer::` 178 (without T-PL.8.13's shards), C6's small shards 4 of 4. These suites ran on
  7cf11b1a, whose engine is d8adcbe2's. The tree landed differs from d8adcbe2 in crates/amphora, crates/line-loom and records alone, which the engine does not depend on.

## Reproduce

```sh
python3 bench/sqlite_outer_join_subqueries.py 300 CH > crates/engine/tests/fixtures/outer_join_subqueries.tsv   # SQLite's answers
cargo test -p amphora-engine --features simulator --test oracle_findings --test outer_join_subqueries
```
