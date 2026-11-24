# Lane C9 (planner), 2026-11-24 08:46:28Z to 09:46:28Z: a LEFT JOIN keeps the left rows its ON refuses (the oracles' finding CG), and its other form, a partial index proven by a LEFT JOIN's ON

Machines: adacachy2 (runner 00aad8da, Linux, 32 cores, load 25 to 59 all hour from other lanes) and, for portable tests, MacBook Pro 2
(runner e30189ea). Every build debug (`simulator` feature unless said), `nice -n 10`. Started on main f183dd75; landed as main af8ca01c
(09:34:13Z; commits f465ddf7 tests, d85f8f1a and ffcc65e0 the fixes, 7fbbfe5d findings.md). Runs named (r...) are in kiln's output log.

## 1. Losses first

- **CG is wrong with statistics too.** C8's shape was right with statistics; on main's engine the sweep has 1 wrong of 640 in `small` and 4 of 240 in `large`
  with ANALYZE, each under a multi-index AND, and the same with ANALYZE and the parallel mode (r1402).
- **A second wrong answer of the same kind was on main**, not the multi-index AND's: an inner-joined table's partial index was taken for proven
  by the ON of a later LEFT JOIN. `a JOIN b ON b.aid = a.id LEFT JOIN c ON b.live = 1 AND c.id = b.id` over `b`'s index `WHERE live = 1`:
  2 rows for SQLite's 4 in the plain plan (the probe on main's engine, four rows of `b`), and where the planner took a hash join, `1, 2, 2` for SQLite's `1, 2, NULL`: the
  ON term counted as answered and was never tested. Of main's 13 wrong of 320 in `partial` at defaults, 10 hold no multi-index AND. The brief names the multi-index AND; this is the second place a left-only ON term filters the left
  table, so it is fixed here too, in its own commit.
- The sweep compares rows as a set. It has no ORDER BY, GROUP BY, DISTINCT, aggregate, USING, derived table or subquery. I ran 10 statements of those kinds by hand
  (a left-only ON equality under ORDER BY, GROUP BY, DISTINCT, min and max, count, LIMIT) at the four settings on the fixed build: 40 of 40 as SQLite.
- With the fix few multi-index ANDs stay at defaults in the sweep (9 of 640 plans in `small`, 3 of 240 in `large`; with statistics 25 and 6): most of what the sweep had
  (21 and 6 at defaults on main's engine) was the bug's. The plans depend on the learned weights and change from run to run; the test asserts only that some occur.
- Not run: a release build, the optimizer's suite in a release build, a benchmark (none was due; no plan is chosen that was not weighed before, but statements
  that took the wrong AND now take another plan).
- Lost time: four runs refused for room (7, 7, 1 and 2 of 32 cores idle), one check run ended at a path pattern of mine (r1444), one had no script (r1435).

## 2. Reproduced

Through SQL on main's engine, the rows beside SQLite 3.54.0. C8's `--replay` tool (`oracle-c8/fixed`) and this lane's probe (perf/planner-c9-probe, not for main).

| statement (`a(id, total REAL, code TEXT UNIQUE, note, k INT)`, indexes on total and on k; `both` is `WHERE a.total > 100 AND a.note IS NULL`) | SQLite | Amphora, default and parallel | with statistics, and both |
|---|---|---|---|
| CG: `a LEFT JOIN b ON a.code = '255' AND b.id = a.id both` | 1,1; 3,NULL | 1,1 | as SQLite |
| an ON on the left table alone, a unique index: `a LEFT JOIN c ON a.code = '255' both` | 1,1; 1,2; 1,3; 3,NULL | 1,1; 1,2; 1,3 | as SQLite |
| an index that is not unique: `a LEFT JOIN c ON a.k = 1 both` | the same | 1,1; 1,2; 1,3 | as SQLite |
| two such terms: `a LEFT JOIN c ON a.code = '255' AND a.k = 1 both` | the same | 1,1; 1,2; 1,3 | as SQLite |
| swapped to a LEFT JOIN: `c RIGHT JOIN a ON a.code = '255' AND c.id = a.id both` | 1,1; 3,NULL | 1,1 | as SQLite |
| made a LEFT JOIN by its WHERE: `a FULL JOIN c ON a.code = '255' AND c.id = a.id both` | 1,1; 3,NULL | 1,1 | as SQLite |
| an OR, not taken: `a LEFT JOIN c ON (a.code = '255' OR a.k = 2) AND c.id = a.id both` | 1,1; 3,3 | as SQLite | as SQLite |

The plan in every wrong one: `MULTI-INDEX AND a (sqlite_autoindex_a_1, a_total)` (or `a_k`, or all three); with statistics `SCAN a`, these tables holding three rows. The oracles' three findings (tlp-0026d1343cdbadae,
tlp-6c50fd752555f572, diff-3fcb32b9317ecd43), replayed with C8's build, each differ from SQLite in one statement at defaults and in parallel mode and in none with statistics. `tests/on_terms.rs` under statistics met it with other shapes (a LEFT JOIN over a RIGHT one, an ON beside an OR).

## 3. Traced

CG's statement on main f183dd75, EXPLAIN bytecode and row counters (`Connection::set_row_counters`) in one run:

| what | main | fixed |
|---|---|---|
| plan | `MULTI-INDEX AND a (sqlite_autoindex_a_1, a_total)` | `SCAN a` |
| the ON's `a.code = '255'` | addresses 7 to 12: `SeekGE` and `IdxGT` on sqlite_autoindex_a_1, its rowids into a RowSet | address 13: `Ne r10 '255'` in b's loop, after b's seek: a left row it refuses goes to the NULL row |
| `a.total > 100` | 13 to 19: `SeekGT` on a_total, each rowid tested against the RowSet | address 6, a filter of a |
| rows of `a`, counted | 1 | 2 |
| rows of `b`, counted | 1 | 2 |

- **First wrong place:** `multi_index_can_consume_term` (optimizer/multi_index.rs) let any term through for a table no outer join null-extends. `a` is the left table of the LEFT
  JOIN, so its ON (`WhereTermOrigin::Join(Outer(b))`) was a candidate branch of `a`'s intersection, and the scan marks the terms it takes consumed: nothing tests the
  ON again. The plain reads do not make this mistake: `constraints_from_where_clause` puts an outer join's ON terms on another table aside (`on_elsewhere`).
- **Fix (d85f8f1a):** the rule by the term's origin: a join's own ON drives its right table's scan; another outer join's ON drives none; a WHERE or inner ON term drives it
  where no outer join null-extends the table.
- **The other form's first wrong place:** `partial_index_predicate_terms` (optimizer/constraints.rs) took every term as proof for a table with an inner join of its own (and for a first
  table only those of no outer join). SQLite's `whereUsablePartialIndex` (where.c) asks `!EP_OuterON || iJoin == iTab`. **Fix (ffcc65e0).**

## 4. Held

`bench/sqlite_on_terms.py` draws 1,200 statements (`small` 640, `partial` 320, `large` 240 over a 3,000-row table and two small ones), two to four tables joined JOIN, LEFT, RIGHT or
FULL, each ON of left-only, right-only and mixed terms, WHERE terms on either side, columns with a unique index, a plain one, a partial one (in `partial`) and none, all with NULLs; half of the
draws put CG's trap at a join. SQLite 3.54.0's rows of each are in tests/fixtures/on_terms.tsv (100 of the 1,200 empty). `tests/on_terms.rs` runs every statement at the four settings, 4,800 runs:

| arm (r1402; the same tests) | small: default, parallel, statistics, both (of 640) | partial (of 320) | large (of 240) | wrong of 4,800 |
|---|---|---|---|---|
| main's engine | 14, 14, 1, 1 | 13, 13, 7, 7 | 5, 5, 4, 4 | 88 |
| the multi-index fix alone | 0, 0, 0, 0 | 12, 12, 9, 9 | 0, 0, 0, 0 | 42 |
| both fixes | 0, 0, 0, 0 | 0, 0, 0, 0 | 0, 0, 0, 0 | 0 |

(The 12 and 9 with one fix against 13 and 7 on main: the plans follow the weights each run learns, so how many statements meet a cause moves between arms; inferred. The landed tree b70fd45a, run again after the landing: 0 of 4,800.)

- `a_left_join_keeps_the_left_rows_an_on_term_of_theirs_refuses` (tests/oracle_findings.rs): CG's query, the forms of section 2 and the oracles' three, four settings. Red on main's engine: `left: ["1|1"]`, `right: ["1|1", "3|NULL"]`.
- `a_left_joins_on_term_proves_no_partial_index_of_a_left_table` (tests/on_terms.rs): red on main's engine and on the multi-index fix alone
  (`left: ["1,1,1", "1,2,2", ...]`, `right: ["1,1,1", "1,2,NULL", ...]`, a hash join).
- `test_multi_index_intersection_takes_the_terms_that_filter_its_table` (multi_index.rs): which origins may drive an intersection, seven cases (left and right table, inner and LEFT JOIN, ON and WHERE).
  Red on main's engine: "item first: true, LeftOuter, on"; `assertion left == right failed`.
- `the_terms_that_filter_a_table_keep_its_multi_index_reads` (tests/on_terms.rs): nine plans at defaults that keep their multi-index AND or OR (a WHERE's two terms, an inner join's ON beside a
  WHERE's, the table's own LEFT JOIN's ON, a WHERE's two beside another table's LEFT JOIN, over an inner join, and the OR forms).

## 5. Checks

Main moved inside crates/engine three times while the checks ran: lane B25 (backup, e69221de), lane C10 (CH, the unmatched-row read, 47b678a3's landing; it and this lane both append a test to
oracle_findings.rs and a row to findings.md: both kept) and lane V22 (7173ca16). Each was merged and the touched tests and clippy run again; the long suites ran once, on cad5832b.

| check | tree | result |
|---|---|---|
| `cargo fmt --all --check` | the landed tree | clean |
| clippy `-p amphora-engine --all-targets --locked -D warnings` | d5468865 (r1450), cad5832b (r1482), e77e79ed (r1557), b70fd45a (r1571) | exit 0 each; "Compiling amphora-engine" in the first, "Checking amphora-engine v0.1.0" in the others |
| `--test on_terms` (this lane's: 4,800 runs of the sweep, the partial index's test, the plans kept) | cad5832b (r1482), e77e79ed (r1557), b70fd45a (r1571) | 5 passed each, 23.0, 17.9 and 15.1 s |
| `--test oracle_findings` | cad5832b: 85, e77e79ed: 86 (with C10's), b70fd45a: 86 | all passed |
| the multi-index unit tests, `translate::optimizer::multi_index` | e77e79ed (r1557), b70fd45a (r1571), and in the suite below | 5 passed each |
| the optimizer suite, `translate::optimizer` less C6's sweep (debug, 4 threads) | cad5832b (r1482) | 182 passed, 0 failed, 292 s |
| C6's small sweep, `outer_joins_answer_as_sqlite_small*` | cad5832b (r1482); e77e79ed (r1557) | 4 passed, 26,880 runs, 59.5 s; 4 passed, 63.0 s at 2 threads |
| `--test join_methods`, `--test adaptive_joins` (simulator), `--test sorted_fetch` | cad5832b (r1482), the first two e77e79ed (r1557) | 4 passed and 1 ignored; 23 passed; 1 passed |
| `--test planner` (simulator) | cad5832b (r1482) | 13 passed, 1 failed: `a_whole_table_group_by_scans_and_sorts_where_lookups_would_read_pages` (planner.rs:182), main's own |
| `--test outer_join_subqueries` (lane C10's) | e77e79ed (r1557), b70fd45a (r1571) | 1 passed each |
| check-production (Linux, adacachy2) | d5468865 (r1450), cad5832b (r1482), e77e79ed (r1557), b70fd45a (r1571) | exit 0 each: 10,617 left, as the ratchet holds (b70fd45a 10,616: lane V22's landing lowered it) |
| the same tests on macOS, MacBook Pro 2 | cad5832b (r1460) | on_terms 5 passed (244.9 s at load 43), oracle_findings 85, join_methods 4, adaptive_joins 23, sorted_fetch 1, planner 13 of 14 (182); the optimizer suite and C6's sweep did not end before the record (my build there took 11 min at that load) |

## 6. Seen, left

- A multi-index AND takes every candidate term, not a subset: with a third term of a wide range on it (`a.code > '1'` beside `a.total = 100 AND a.k = 1`, the LEFT JOIN made inner) the AND is dearer than
  one index, and the plan reads one (an estimate and cost matter; no wrong rows).
- A partial index whose filter the query's own WHERE or inner ON proves, under a hash join: 7 statements on the fixed build gave SQLite's rows at defaults.
- `--test planner` (planner.rs:182) is red as on main.

## 7. Reproduce

```sh
python3 bench/sqlite_on_terms.py | cmp - crates/engine/tests/fixtures/on_terms.tsv      # the fixture, SQLite 3.54.0
python3 bench/sqlite_on_terms.py --rows small '<query>'                                 # SQLite's rows of one query
cargo test -p amphora-engine --test on_terms -- --nocapture                             # the sweep, its plans counted
cargo test -p amphora-engine --test oracle_findings a_left_join_keeps
cargo test -p amphora-engine --lib translate::optimizer::multi_index
amphora-bench --oracle --sqlite LIB --replay bench/records/2026-11-24-adacachy2-oracle-c8/findings/tlp-0026d1343cdbadae.md
```
