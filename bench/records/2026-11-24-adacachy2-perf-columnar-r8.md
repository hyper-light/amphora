# Lane R8: column copies declared per table, and why each of S8's statements skips them (2026-11-24, adacachy2)

Ada's goal is Amphora about ten times faster than DuckDB and SQLite on analytics. R7 found that the
column copies weighed none of S8's five analytics statements, and that the copies ship behind a
process-wide switch where decision 1 of `docs/design/columnar.md` says they are declared on the
table. This lane had one deliverable: the per-table declaration, and the reason each S8 statement
skips the copies. Nothing here makes a statement faster; the reasons are the next lane's work.

Verdict, losses first.
- None of S8's five statements can reach a copy today, declared or not. In both runs three never
  get the copies' hook from the translator (table_count, aggregates_range, join_three) and
  filter_no_index gets it and is skipped as `body` (a shape only the statement's own loop states).
  table_group_by was skipped as `body` in r13 and got no hook in r32, where the planner read it by
  an index instead. So declaring S8's tables changes none of its numbers.
- `cb4_ab_both_rebuy_policies_at_the_extremes` still fails at its waves check (`dropped` 0), as on
  main before this lane (R7's record, section 1). The brief sets it aside; not touched here.
- `cargo fmt --all --check` fails on main itself, in files this lane didn't touch (`ieee754.rs`,
  `log_segments.rs`, others); the files this lane touched are formatted (r32).
- The declaration is made at CREATE TABLE only: no ALTER form is built (decision 1 says so).
- Built and merged; the declaration is live in no deployment.

## 1. The declaration

`CREATE TABLE t (...) COLUMNAR` (or `STRICT, COLUMNAR`): a table option, SQLite's own form for what a
table is (`STRICT`, `WITHOUT ROWID`), kept in `sqlite_schema`'s text and so read back at every open.
Decision 1, item 1 now says so, with the reasons.

- Parser: `TableOptions::columnar_text`, the `COLUMNAR` arm beside `STRICT`, and its writing back
  (`crates/engine-parser`). Schema: `BTreeTable::columnar`, `BTreeCharacteristics::COLUMNAR`, and
  `to_sql` writes it (`crates/engine/schema.rs`), so an ALTER that rewrites the CREATE keeps it.
- Translator: the hook's purpose is a type, `MorselHookFor { Off, Morsels, Copies }`
  (`translate/emitter/mod.rs`). On the versioned storage it is `Copies`, and `emit_morsel_hook`
  emits the hook only when the table it reads is declared (`translate/parallel.rs`, `declared`).
- Deleted: `column_path::ENV` and `asked` (`AMPHORA_COLUMN_PATH`), `Copies::on` and `set_on`,
  `Storage::set_copies_on`. Moved to the declaration: every copies test in `columns_sql.rs` (a
  `create` helper declares the table on the copies' side), `bench/src/colcopy.rs` (its Amphora
  declares its tables when it keeps copies), and the suite's new `--columnar` (amphora target only)
  in place of the environment variable.
- Test: `cs5_only_a_table_declared_columnar_keeps_a_copy`. Two tables alike, one declared, the
  same statements three times on each: the same answers, one copy kept, the declared table's, and
  the declaration kept in its CREATE text.

## 2. Why each S8 statement skips the copies

`r8_s8_statements_on_declared_tables_say_why_they_skip_the_copy` (`columns_sql.rs`) builds S8's
three tables and five indexes as the suite does (`bench/src/suite.rs`), declared `COLUMNAR`, with
3,000 items, binds each statement's parameters as the suite binds them, runs it three times, and
prints whether its program holds the hook (`MorselAggregate` in its EXPLAIN), its plan (EXPLAIN
QUERY PLAN), and what the copies counted. Answers equal the same tables undeclared. r13
(b0d07f3e) and r32 (6bbedbf7, with `AMPHORA_PLAN_TRACE=1`, which shows the copies' trace events):

| statement | hook | plan | copies' counts (3 runs) | the code that declines it |
|---|---|---|---|---|
| table_count | no | `SCAN items USING COVERING INDEX items_owner` | none handed | `translate/parallel.rs` `shape` (`Scan::BTreeTable { index: None, .. }`) and `scanned_rowids` (the same for `body_plan`): the planner reads COUNT(*) from a covering index, and the hook takes only a scan of the table itself |
| table_group_by | r13: yes; r32: no | r13: `SCAN items`, `USE HASH TABLE FOR GROUP BY`; r32: `SCAN items USING INDEX items_grp_created` | r13: handed 3, skipped `body` 3; r32: none handed | the planner chose differently in the two runs. With the table scan (r13): `storage/versioned/columns_sql.rs` `aggregate` (`Some(body) if !body.simple => Some(Skip::Body)`), because the simple shape refused it in `parallel.rs` `outputs` (AVG is none of COUNT, SUM over integers, MIN, MAX) and AVG(price) reads a REAL column, which `shape`'s `column` refuses. With the index scan (r32): `parallel.rs` `shape` and `scanned_rowids`, as for table_count |
| filter_no_index | yes | `SCAN items` | handed 3, skipped `body` 3 | the same `Skip::Body` arm: the simple shape refused its terms in `parallel.rs` `predicate`, which takes a column compared with a literal of its own kind; `qty = ?` compares with a parameter and `price < ?` with a REAL column |
| aggregates_range | no | `SEARCH items USING INTEGER PRIMARY KEY (rowid>=? AND rowid<=?)` | none handed | `parallel.rs` `scanned_rowids`: a rowid range is taken only between integer literals (`SeekKeyComponent::Expr(expr) => integer(expr)`); its bounds are parameters |
| join_three | no | `SEARCH i` by a rowid range, `SEARCH o` and `SEARCH r` by rowid | none handed | `parallel.rs` `shape_join` and `shape_chain` (`_ => return None`): the first table's range is between parameters, and the others are read by a seek per joined row, which only the searched-links switch takes (off by default) |

The trace lines (r32):

```
r32, cs5_only_a_table_declared_columnar_keeps_a_copy (the declared table's copy bought, then chosen):
DEBUG amphora::columns: column copy bought tree=3 rebuy=false
DEBUG amphora::column_path: column copy or rows: the choice and its two costs tree=3 rows=500 queued=0 exact=true copy_nanos_per_row=Some(186.908) rows_nanos_per_row=5964.696 rows_cost=2982348.0 copy_cost=Some(93454.0) path=Copy

r32, r8_s8_statements_on_declared_tables_say_why_they_skip_the_copy:
== r8 table_count: hook in its program false; plan ["SCAN items USING COVERING INDEX items_owner"]; handed 0, skipped rows/body/join/transaction 0/0/0/0, read by the copy 0, sent to the rows 0, no copy yet 0, copies kept 0
== r8 table_group_by: hook in its program false; plan ["SCAN items USING INDEX items_grp_created"]; handed 0, skipped rows/body/join/transaction 0/0/0/0, read by the copy 0, sent to the rows 0, no copy yet 0, copies kept 0
DEBUG amphora::columns: column copy skipped: no copy can answer the statement reason="body"   (three times, filter_no_index)
== r8 filter_no_index: hook in its program true; plan ["SCAN items"]; handed 3, skipped rows/body/join/transaction 0/3/0/0, read by the copy 0, sent to the rows 0, no copy yet 0, copies kept 0
== r8 aggregates_range: hook in its program false; plan ["SEARCH items USING INTEGER PRIMARY KEY (rowid>=? AND rowid<=?)"]; handed 0, skipped rows/body/join/transaction 0/0/0/0, read by the copy 0, sent to the rows 0, no copy yet 0, copies kept 0
== r8 join_three: hook in its program false; plan ["SEARCH i USING INTEGER PRIMARY KEY (rowid>=? AND rowid<=?)", "SEARCH o USING INTEGER PRIMARY KEY (rowid=?)", "SEARCH r USING INTEGER PRIMARY KEY (rowid=?)", "USE HASH TABLE FOR GROUP BY"]; handed 0, ...

r13 (no trace subscriber; the copies' counts):
== r8 table_group_by: hook in its program true; plan ["SCAN items", "USE HASH TABLE FOR GROUP BY"]; handed 3, skipped rows/body/join/transaction 0/3/0/0, read by the copy 0, sent to the rows 0, no copy yet 0, copies kept 0
```

A statement the translator gives no hook has no copies' event to show: its absence from the program
(EXPLAIN) and its plan are the trace. Why table_group_by's plan moved between r13 and r32 (main
merged in between, and the planner prices by costs it measures) is not looked into here.

What the next lane inherits: three of the five need the translator to hand the copies a shape it
refuses today (a covering-index scan, a rowid range between parameters, seeks per joined row), and
the other two need the copy to answer what only the loop body states (AVG, REAL columns, terms
against parameters).

## Runs

- r13 (adacachy2, needs `cpus=4,mem=12g` after `cpus=8,mem=24g` was refused with 5 of 32 cores
  idle; 02:04Z to 02:09Z, b0d07f3e): the S8 test (above); the copies, path, scan, schema and
  parallel tests, 104 passed, 1 failed (`cb4_ab`, above); the parser's 33 passed; clippy on
  `amphora-engine` and `amphora-engine-parser`, all targets, exit 0; `scripts/check-production.sh`
  exit 0 (the engine's ratchet holds at 10,903); the bench's `cargo check --release` finished.
- r32 (adacachy2, `cpus=4,mem=12g`, 02:09Z to 02:14Z, 6bbedbf7: the lane merged with main): the S8
  test and the declaration test traced (above), both pass; the same suites, 104 passed, 1 failed
  (`cb4_ab`, the same waves assertion); the parser's 33 passed; `cargo fmt --check` finds nothing
  in the files this lane touched; clippy on both crates exit 0; `scripts/check-production.sh` exit 0
  (ratchet 10,903); `columnar_decision` (T-CS.1.1) fails, and fails the same on main's own
  `columnar.md` (r35 below): its faults are all numbers in decision 2, item 4, none in this lane's
  text.
- r35 (adacachy2, the same tree): `columnar_decision` on this lane's `columnar.md` and on main's
  (9dfd4cf4): main's own doc fails it at decision 2, item 4's ladder and table counts.
- r16 (macOS, a second copy of the checks): stopped before it started, once r13 had answered.
