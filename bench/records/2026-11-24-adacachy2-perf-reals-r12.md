# Lane R12: S8's aggregates_range reaches the column copy, with SQLite's answers (2026-11-24, adacachy2)

Ada's goal is Amphora about ten times faster than DuckDB and SQLite on analytics, with answers
SQLite can't tell from its own. This lane's deliverable: a rowid range between parameters reaches the
copy, every aggregate aggregates_range uses is taken by the copy or handed back with SQLite's answer,
checked against sqlite3 3.54.0, and merged. It carries on lane R11's unfinished work.

Verdict, losses first.
- aggregates_range is not answered by the copy under `RealSums::Serial`, the default. It is
  `SUM(price), AVG(qty), MIN(created), MAX(created)` and `price` is REAL, so every granule holds a REAL
  to sum, and the copy declines the statement (as R10's). The S8 trace shows `path=Copy` for it, three
  of three; the copy read it 0 times, and the rows answered. `price` in S8's `items` is REAL, so under
  Serial this is every run.
- The chooser prices a rowid range as the whole table. `aggregate_choice` (columns_path.rs) gives
  `Shape::Range { rows: table_rows }`, and says so ("a key range by the rows the copy holds, the most
  it can read (no estimate of the range's rows is made here)"). Reading one row by its rowid is priced
  like reading the table, on both sides. Before this lane the hook refused such ranges, and the serial
  loop sought. In r368's r11 run the copy read 393 of 600, ranges of one row among them. No A/B.
- `aggregate_on_copy` charges a scan's time to the table's rows (`view.rows`), not to the rows the
  scan visited. A range over a larger table than r11's 3,002 rows, reading one of its granules, would
  measure the copy as cheap as the table is large. In r11 (3,002 rows, all read by any range) the
  measured rate stays 93 to 150 ns a row for ranges and whole scans. Not measured larger. Not changed.
- The rowid range now reaches the morsel worker's leaves (the pager's storage, `paged` in the test), but
  no counter asserts how many leaves are read; only the answers are.
- SUM and AVG over a REAL column now ask the copy before the serial loop, where the translator used to
  refuse them. Under Serial the copy always hands them back; the cost of asking is unmeasured (no A/B).
- The suites the brief asked for (columns, path, scan, morsel, parallel) are not a result: they were still
  running at 2 threads on a loaded adacachy2. The focused tests, clippy and check-production are. The rule
  5b of the swarm's RULES puts the suites after the merge, on main, with a regression fixed forward.
- A test fails on the landed tree: `vdbe::morsel_scan::tests::t_qt_6_35_joins_over_rowid_ranges_run_on_
  morsels_as_the_serial_loop` (morsel_scan_tests.rs:3504), "SELECT count(*), sum(d.b) FROM f JOIN d ON
  f.a = d.a WHERE f.id > 2000 on 1 cores: the hook built no link". Found after landing, alone on the merged
  binary 7659d3e1 (r509's build) and in r368's suites at c62c8d05. Whether it fails on main before this
  lane is not known: not bisected. The lane's diff doesn't touch the join path (`shape_join`), and R10's
  suites never ran that module (`vdbe::morsel_scan_tests::` is not its path).
- Lane R11 left work that broke what R10 had landed. At R11's state (cb704058, merged with main
  a64549cc), `rowid_range` turned every bound parameter into a key range from i64::MIN to i64::MAX. The
  copy prices an unmeasured range as no path, so no statement with a parameter read the copy: r10 and
  r11 failed on "the copy read some" (r323, 1,138 choices of `path=Rows`). Fixed in c62c8d05, with r10
  and r11 passing after.

## 1. What changed

- R11's (2ac29b10): `translate/parallel.rs` turns a forward search of the table b-tree by a rowid range
  into terms on the rowid's alias column (`rowid_bounds`), each bound an integer literal or a bound
  parameter. `vdbe/execute.rs` binds the terms when the statement runs, and `morsel::rowid_range`
  narrows the rowids the scan reads to those they admit: to the copy's key range (`columns_sql.rs`,
  `query`) and to the leaves the morsels are cut from (`TableSource::restrict`, the pager's storage).
  The terms still decide each row, so the range may be wider than they are, never narrower. SUM and AVG
  over an INTEGER or a REAL column are in the simple shape; the copy has AVG (`Agg::Avg`), which declines
  a sum past a double's exact integers (2^53) to the rows. The pager's workers hand back a SUM that meets
  a REAL under Serial (`TableAggregate::sums_a_real`).
- This lane's (c62c8d05): an end no term bounds is None, and the whole table is a whole-table scan (a
  range at both ends of the i64s, and a text or a blob as an upper bound, bound nothing). The
  documentation cites SQLite's source for each bound type (parallel.rs, morsel.rs). The S8 test
  (`r8_s8_...`) turns on the copies' trace itself.

Each bound type, as SQLite 3.54.0 compares it (`vdbe.c` OP_SeekGE and OP_SeekGT on a table cursor;
`wherecode.c` `sqlite3WhereCodeOneLoopStart` case 3, the end's OP_Gt or OP_Ge with
`SQLITE_AFF_NUMERIC | SQLITE_JUMPIFNULL`):

| Bound | As a start (`>=`, `>`) | As an end (`<=`, `<`) |
|---|---|---|
| integer | itself | itself |
| real | `sqlite3IntFloatCompare` of it with the integer it truncates to moves the seek past it (4.5 seeks 5); past 2^63 none | compared by value, exactly |
| text reading as a number | that number (`applyNumericAffinity`) | that number |
| other text, a blob | no row (`jump_to_p2`) | above every rowid: none past it |
| NULL | no row | no row (`JUMPIFNULL`) |

## 2. Checked against SQLite 3.54.0

- `bench/sqlite_answers_r10.py` (extended by R11, run here by this machine's Python 3, sqlite3 3.54.0)
  prints 100 range cases after `// R11`: both ends, empty ranges, reversed bounds, one row, each bound
  type as a start and as an end (integers, reals including -0.0 and past 2^63, numeric text with spaces
  and exponents, text that reads as no number, blobs and NULL), over S8's aggregates_range and four other
  statements whose aggregates the copy takes or hands back. The test holds exactly those 100 (compared
  by script: 0 missing, 0 extra).
- `r11_a_rowid_range_between_parameters_reaches_the_copies_with_sqlites_answers` runs each on a
  declared table, an undeclared one and the pager's storage, under `ParallelMode::Off` (one worker) and
  `On` (the workers the switch picks), three runs each on the versioned storages, under `RealSums::Serial`.
- r368 (adacachy2, `cpus=2,mem=6g`, c62c8d05, load 31 to 66): passed. r11 handed 600, read by the copy
  393, sent to the rows 101, no copy yet 1. r10 handed 540, read by the copy 484 (R10's numbers), r9
  handed 18, read by the copy 16. S8 (r8): aggregates_range `path=Copy` 3 of 3, read by the copy 0 (the
  decline above); filter_no_index read by the copy 3; clippy exit 0 in 49 s ("Checking amphora-engine").

## 3. Runs

- r323 (adacachy2, `cpus=8,mem=24g`, cb704058, load 38): build exit 0 in 63 s; focused tests failed
  (r10, r11) as above. Clippy exit 0. Stopped after clippy to fix the bug.
- r353, r356 (adacachy2, `cpus=8` then `cpus=4`): refused, 1 and 2 of 32 cores idle.
- r361 (macOS, `cpus=8,mem=24g`): queued, stopped before it ran, as the Linux focused tests had passed.
- r368 (adacachy2, `cpus=2,mem=6g`, c62c8d05): focused tests and clippy above. Its suites
  (`columns_sql::`, `columns_path::`, `columns::scan::`, `vdbe::morsel::`, `vdbe::morsel_scan_tests::`,
  `translate::parallel::`, 2 threads) were still running after 30 minutes when this lane landed, 88 tests
  done at 21 minutes: not a result. Its checkout was moved under it at 05:48Z (r503, stopped at once), so
  its remaining steps are of another tree: don't read them.
- r420, r431, r433 (check-production, `cpus=4`, `8`, `4`): refused, adacachy2 at 2, 6 and 3 of 32 cores
  idle; r435 was lost ("the kiln server did not answer in time"), r438 ran it.
- r438 (adacachy2, `cpus=2,mem=6g`, 374336fd, the lane merged with main 3a58a13f, in its own worktree):
  check-production exit 0 in under a minute, "10903 left in the engine's production code across 17 lints,
  as the ratchet holds".
- r509 (adacachy2, `cpus=2,mem=6g`, 7659d3e1, the lane merged with main 219603df, which moved crates/engine
  under it, in its own worktree): the focused tests passed (r10 484 of 540 by the copy, r11 393 of 600, r9
  16 of 18, S8's aggregates_range `path=Copy` 3 of 3, filter_no_index read by the copy 3 of 3); clippy
  exit 0 in 43 s ("Compiling amphora-engine", which rule 6 counts as re-checked).
- Landing: main had moved from 3a58a13f (the base of r438) to 219603df (crates/engine, among them
  execute.rs and insn.rs, which the lane also changes), and then to c205dd7d. The lane merged both, and
  checked 219603df's tree (r509, above). c205dd7d added only `crates/amphora` (replicated.rs), which
  amphora-engine does not depend on (`cargo tree -p amphora-engine`), and records, so it landed without a
  re-run. check-production was run on 374336fd, whose engine differs from the landed tree's only in
  main's later engine changes and comments.

## 4. What the next lane inherits

- A/B of aggregates_range and of every statement that now asks the copy (R10's list): the copy is chosen
  and then declines, for SUM and AVG over REAL, under Serial.
- The chooser's range estimate and the copy's per-row rate on ranges (the second and third losses above).
- join_three, table_count, table_group_by, MIN and MAX and GROUP BY over REAL: not this lane's.
