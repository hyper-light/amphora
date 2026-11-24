# Lane R9: bound parameters in the column copy's filter (2026-11-24, MacBook Pro 2)

Ada's goal is Amphora about ten times faster than DuckDB and SQLite on analytics. R8 found that none
of S8's five analytics statements can reach a COLUMNAR table's column copy, and two of the reasons
were the same: the copy path takes only literals. This lane's deliverable was statements with bound
parameters reaching the copies, merged to main.

Verdict, losses first.
- The deliverable is not done, and nothing is merged to main. One of its parts is built, on branch
  `perf/columnar-r9` (dbfd4d9d): a term comparing an INTEGER column with a bound parameter is in the
  copy's simple shape, bound when the statement runs. Not built: the rowid range between
  parameters, AVG and REAL columns in the simple shape, their tests, and the A/B.
- No S8 statement reaches a copy with this change alone. filter_no_index's `qty = ?` is now taken,
  but its other term, `price < ?`, compares a REAL column, which the simple shape still refuses
  (section 2), so the statement is still skipped as `body`.
- No A/B: nothing it would measure changed.
- The hour lost about 15 minutes to kiln's restart on adacachy (02:18Z). adacachy2 refused the
  check twice (2 and 3 of 32 cores idle), and kiln's sync to MacBook Pro 2 timed out twice. The check
  then ran from a GitHub clone on MacBook Pro 2 with a cold target dir (r58, below).

## 1. What changed (dbfd4d9d)

- `Predicate::Parameter { column, op, index, affinity }` (`vdbe/morsel.rs`): `column op ?` where the
  column has INTEGER affinity. The translator (`translate/parallel.rs`, `parameter`) takes it only when
  the serial loop's comparison affinity (`comparison_affinity`) is NUMERIC. For an INTEGER column
  against a parameter, which has no affinity, it always is (SQLite's datatype3, section 4.2).
- `op_morsel_aggregate` binds each such term before the column copy or the morsels read the filter
  (`Predicate::bind`). The parameter's value is read from the statement's parameters (NULL when
  unbound, as `Variable` reads it) and converted by `Affinity::convert_for_compare`, the serial
  loop's own conversion: a numeric text becomes its number, and other values stay as they are.
- The column's side needs no conversion. An INTEGER column's stored text is text that numeric
  affinity left as text when it was stored, and the comparison applies the same function.
- An unbound `Parameter` reaching either pipeline is an error, never a wrong answer
  (`Predicate::holds`, `columns_sql::query`).
- Test `r9_a_parameter_compared_with_an_integer_column_reaches_the_copies` (`columns_sql.rs`) uses
  the same table declared and not declared, with an integer, a real, a text, a blob and a NULL in
  the compared column. It binds an integer, a real, a numeric text, a non-numeric text, a blob and
  NULL, three runs each. It expects equal rows on both tables, every run handed to the copies and
  none skipped as `body`, and the numeric text answering as its number.

## 2. What the next lane inherits (read from the code this lane, not measured)

- REAL columns. SQLite stores an integral REAL as an integer (`sqlite3VdbeIntegerAffinity` in
  OP_MakeRecord), and the serial loop reads it back as a REAL (RealAffinity). The copy's decoder
  (`SqlRows`, `columns_sql.rs`) keeps it as `Cell::Int`. Comparisons agree except in one case. An
  integer that no double holds exactly, stored in a REAL column, is kept as that integer, while the
  serial loop reads it rounded. MIN, MAX and GROUP BY values would come out INTEGER where the
  serial loop gives REAL. The copy has to convert REAL-affinity columns as it decodes them, from the
  table's schema, not from the statement: granules are cached per generation, so every statement on
  a table must decode alike. The pager's simple pipeline (`TableAggregate`) has the same gap.
- SUM and AVG over REAL. The copy's `Acc::Sum` adds REALs into an `ExactSum` (exact, rounded
  once). That is `RealSums::Exact`'s answer (arm b), not `Serial`'s, which is SQLite's KBN in row
  order. Under `Serial` the copy needs either one KBN pass in row order (partial sums can't be merged
  bit for bit) or to hand REAL sums to the rows. The same gap exists today for an INTEGER column
  that holds a REAL value. No scenario was run for it here.
- AVG. The copy's `Agg` has no AVG.
- A rowid range between parameters. `shape` takes only a whole-table scan, and `scanned_rowids` takes
  integer literals only. With parameters, the bounds have to be worked out when the statement runs,
  by SQLite's rules for comparing a rowid with the bound value:
  - NUMERIC affinity is applied to the value, and a REAL bound rounds inward.
  - A NULL bound means no rows.
  - A TEXT or BLOB bound that reads as no number sorts above every integer. It gives no rows as a
    lower bound and every row as an upper bound.

  The bounds then go to the copy's `Query.keys` (`KeyRange` tests every key, so exact integer bounds
  make the range exact) and to the pager's `TableSource::restrict`.

## Runs

- r21 (macOS warm-up of main): stopped before it started. Its placement pointed at 2768d0d5, which
  RULES.md forbids.
- adacachy2, `cpus=8,mem=24g` then `cpus=4,mem=12g`: refused, with 2 and then 3 of 32 cores idle.
- r42 and r53 (MacBook Pro 2, kiln's sync): "did not answer the sync in 60s", twice.
- r58 (MacBook Pro 2, a GitHub clone at `~/.cache/amphora-perf/reals-mac`, cold target dir
  `~/.cache/amphora-perf-target/columnar`, load 46 to 61 at the start, 02:47:56Z, dbfd4d9d): two
  compile errors, fixed in a205637f. `ValueRef::to_owned`'s error is the engine's own
  `TryReserveError`, and `morsel_scan_tests`' match over `Predicate` lacked the new variant.
- r83 (a205637f): stopped once main had moved inside `crates/engine`, so the merged tree was checked
  instead.
- r85 (MacBook Pro 2, `cpus=4,mem=12g` after `cpus=8` was refused at 7 of 18 idle cores, e8fccd87 =
  the lane merged with main 7de5344d, load 55 to 67): with `AMPHORA_PLAN_TRACE=1`, both
  `r9_a_parameter_compared_with_an_integer_column_reaches_the_copies` and
  `r8_s8_statements_on_declared_tables_say_why_they_skip_the_copy` pass.
  - R9's test: 18 runs handed to the copies, 16 read by the copy, 1 sent to the rows, 1 with no copy
    yet, 1 copy kept, none skipped. The trace's first choice is `path=Rows` (no generation answered
    the statement's snapshot), then `path=Copy` 17 times.
  - In the first copy choice, the rows are priced at 23,675 ns per row of the table. The copy is
    then measured at 165 to 857 ns per row (`copy_nanos_per_row`).
  - S8's reasons are R8's: filter_no_index and table_group_by are skipped as `body`, and
    table_count, aggregates_range and join_three get no hook.
