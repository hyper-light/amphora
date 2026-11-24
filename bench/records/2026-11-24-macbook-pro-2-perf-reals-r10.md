# Lane R10: R9 landed, and REAL columns in the column copy's filter (2026-11-24, MacBook Pro 2 and adacachy2)

Ada's goal is Amphora about ten times faster than DuckDB and SQLite on analytics, with answers
SQLite can't tell from its own. This lane's deliverable: R9's bound parameters on main, and REAL
columns through the copies with SQLite's answers.

Verdict, losses first.
- SUM over REAL is handed back to the serial loop, not summed in the copy. The brief asked for the
  choice "from a measurement". What was measured is that the hand-back gives SQLite's bits (below).
  What a row-order KBN pass in the copy would save was not measured: that is an A/B, the next lane's.
- SUM over a REAL *column* still doesn't reach the copy: the translator takes SUM only over INTEGER
  columns, as before. Only a stray REAL in an INTEGER column reaches the copy's SUM, and it is
  handed back. MIN, MAX, COUNT(col) and GROUP BY over a REAL column are still refused (not asked).
- `cb4_ab_both_rebuy_policies_at_the_extremes` fails, and it fails on main too: on 7de5344d (main
  before R9, r197), on 9fbfdbaf (main with R9, r183) and on the lane (r148, r183, r135), always the
  same line. Its "both at fleet scale" scenario under `A_rent` drops no copy (`dropped: 0`, asked
  320, granted 320, build_rows 6528), and the test expects at least one under `Press::Waves`. Its
  tables have no REAL column and its statements no parameter. It passes at 2ae4401e (r213), the
  lane merged with main 2571f841, whose lane D4 landing changed `columns_ab.rs` and `columns_sql.rs`.
- check-production failed once (r148): `bool::then_some` is a disallowed method, and the engine's
  ratchet counted 41 against 40. Fixed in caabee3d.

## 1. R9 landed

- r85 (R9's run on e8fccd87) ended in clippy: `explicit_auto_deref` at `vdbe/execute.rs:21969`
  (`&**data`). Fixed in b6d7c978. r85 then timed out in its suites and never reported them.
- r122 (MacBook Pro 2, `cpus=4,mem=12g`, b6d7c978, load 71 to 127): `cargo clippy -p
  amphora-engine --all-targets -- -D warnings` exit 0 in 16 min 18 s.
- R9's test file wasn't rustfmt-clean, so 9fbfdbaf formats it. Main moved 7de5344d..9fbfdbaf
  (fast-forward). R9's test passed in r85 on e8fccd87, the same code.

## 2. What changed (a4fdde43, caabee3d)

- `MorselAggregateData::reals`: the table's columns of REAL affinity, from its schema. The copy's
  decoder (`SqlRows`) and the pager's simple pipeline (`TableSource::read_reals`) read an INTEGER
  stored in one as a REAL, as the serial loop's `RealAffinity` does (`i as f64`, as
  `apply_real_affinity`). Every statement on a table carries the same list, so cached blocks decode
  alike for all of them.
- The simple shape takes `column op ?` against a REAL column (BINARY collation), as R9 took an
  INTEGER one. The comparison's affinity is NUMERIC for both (`do_comparison_affinity`), and
  `Predicate::bind` converts the parameter by it when the statement runs.
- The copy declines a SUM whose column holds a REAL in any granule unless `RealSums::Exact`. Under
  `Serial` (and `RowOrder`) the answer is SQLite's compensated sum in row order, which the serial
  loop takes, as the morsels' workers already hand it back (`no_real_outputs`).

## 3. Checked against SQLite 3.54.0

`bench/sqlite_answers_r10.py` builds the table in Python's sqlite3 (SQLite 3.54.0) and prints each
statement's answer as bits (`r<f64 bits>`, `i<int>`, `n`). The test
`r10_real_columns_reach_the_copies_with_sqlites_answers` holds those 90 cases and runs each on a
declared table (three runs), an undeclared one (three runs) and the pager's storage (one run), under
`ParallelMode::Off` and `On`.

- Rows: whole-number REALs (stored as integers), halves, -0.0 in both columns, both f64::MAX, the
  least subnormal, 2^53 + 1 and both i64 ends as integers in the REAL column (read as their nearest
  doubles), a text and a NULL. The INTEGER column holds stray REALs: 2.5, and f64::MAX twice so a
  SUM overflows to inf.
- Parameters: integers, reals, -0.0, numeric text, '1e400', text that reads as no number, a blob,
  NULL, the least subnormal. Operators `<`, `>=`, `=`, `<>` flipped, and S8's `q = ? AND p < ?`.
- r148 (adacachy2, `cpus=8,mem=24g`, 8e3685e2, the same tree as a4fdde43, load 24 to 51): passed.
  Handed 540 runs, read by the copy 484, sent to the rows 1, no copy yet 1. The other 54 are the
  SUM statements, handed back. The trace chose `path=Copy` 557 times and `path=Rows` 3 times over the
  three focused tests.
- S8 (`r8_s8_statements_on_declared_tables_say_why_they_skip_the_copy`, same run): filter_no_index
  now has the hook in its program, is handed 3 times and never skipped as `body`. The trace shows
  `path=Rows` once (not yet timed) and `path=Copy` once. The other four statements are as R8 found.
- `r9_a_parameter_compared_with_an_integer_column_reaches_the_copies` passed in the same run.

## 4. Runs

- r135 (MacBook Pro 2, `cpus=4,mem=12g`, a4fdde43, load 37 to 92): clippy exit 0 at 03:46:18Z.
  The focused tests passed on macOS too (handed 540, read by the copy 484). Suites 39 passed, 1
  failed (cb4, as on Linux). check-production exit 1, the ratchet's `then_some` (fixed after).
- r148 (adacachy2, `cpus=8,mem=24g`, 8e3685e2): check-production exit 1 (the ratchet, above).
  Focused tests passed. Suites (`columns_sql::`, `columns_path::`, `columns::scan::`,
  `vdbe::morsel::`, `vdbe::morsel_scan_tests::`, `translate::parallel::`): 39 passed, 1 failed,
  `cb4_ab_both_rebuy_policies_at_the_extremes` at `columns_sql.rs:2665` (`dropped > 0` under
  `Press::Waves`).
- r178, then r183's first ask: refused, adacachy2 had 0 and then 3 of 32 cores idle.
- r183 (adacachy2, `cpus=3,mem=9g`, caabee3d): check-production exit 0, "10903 left in the engine's
  production code across 17 lints, as the ratchet holds". cb4 fails on caabee3d and on main 9fbfdbaf.
- r197 (adacachy2, `cpus=3,mem=9g`): cb4 fails on 7de5344d, main before R9, at the same assertion.
- r200 (adacachy2, `cpus=4,mem=12g`, ad4db46e = the lane merged with main c67455f6, which moved in
  crates/engine with T19's Postgres dialect): clippy exit 0, the three focused tests passed (r10:
  handed 540, read by the copy 484; S8 filter_no_index `path=Rows` once, `path=Copy` once),
  check-production exit 0 with the ratchet holding.
- r213 (adacachy2, `cpus=4,mem=12g`, 2ae4401e = the lane merged with main 2571f841): clippy exit 0,
  the focused tests passed (the same counts), check-production exit 0, and cb4 passed.
- Landed: main 39ecf1df..6f541481. Main had moved only in `crates/amphora`, which amphora-engine
  doesn't depend on (`cargo tree -p amphora-engine -i amphora`: no path), so no re-run.

## 5. What the next lane inherits

- AVG, and a rowid range between parameters (R9's section 2 still holds for both).
- SUM over a REAL column into the simple shape: under `Serial` it would always be handed back, so it
  only pays under `Exact` or with a row-order KBN pass in the copy. Deciding that is an A/B.
- MIN, MAX and GROUP BY over a REAL column: the decoders now read it as the serial loop does, but the
  shape still refuses it. GROUP BY needs -0.0 and 0.0 in one group.
