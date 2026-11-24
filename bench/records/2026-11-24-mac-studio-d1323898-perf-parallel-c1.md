# Lane C1 (parallel), 2026-11-24 05:47Z to 06:47Z: an ORDER BY the planner took out, and why t_qt_6_35 is red

## Verdict, losses first

- **`t_qt_6_35` is still red, and is not a wrong answer.** It fails on "the hook built no link": the
  parallel path is not taken for its first statement. Its answers agree with the serial loop's and
  SQLite's. It was red at the first commit that compiled it. Not changed (the planner's lane owns the
  plan).
- **check-production was not run on Linux.** adacachy2 was Ada's benchmark window. On macOS the
  engine's counts are the same on main and here, lint by lint.
- **A larger table and a file were not run.** The probe takes both (`C1_SCALE`, `C1_FILE`); the hour
  went to the wrong answer below. Its 393 statements ran at t_qt_6_35's sizes, in memory.
- **A wrong answer on main, found and fixed (T-QT.6.36), in two places a user reaches:**
  - A COLUMNAR table's column copy, with no parallel mode: `GROUP BY s ORDER BY s DESC` (and NULLS
    LAST, NULLS FIRST) came out ascending.
  - Parallel mode (off by default): 1,020 of the 7,930 answers the workers gave were in the wrong
    order.
  In every one the rows were SQLite's, in another order: none missing, extra or changed. The serial
  loop gave none of them (12,766 answers).

## The wrong answer

Statements such as (f has 3,000 rows, d 600, as t_qt_6_35's tables):

    SELECT f.id FROM f LEFT JOIN d ON f.a = d.a WHERE f.id > 2940 ORDER BY f.id
    SELECT d.id FROM f JOIN d ON f.a = d.a WHERE d.id > 560 ORDER BY d.id DESC
    SELECT d.id, count(*), min(f.b) FROM f JOIN d ON f.a = d.a WHERE f.id > 2940 GROUP BY d.id ORDER BY d.id DESC
    SELECT d.c, count(*), min(f.b) FROM f LEFT JOIN d ON f.a = d.a GROUP BY d.c ORDER BY d.c NULLS LAST

**First place it decides differently.** The planner takes the ORDER BY out of the plan: in
`apply_table_access_plan` when the loops give the order (`order_by.clear()`), and in
`compute_order_target` when the GROUP BY can (it leaves the directions in `GroupBy::sort_order` and
`nulls_order`). The morsel hook runs neither the loops nor that GROUP BY's sort. It read only
`plan.order_by`, now empty:

- a join's workers gave their rows in the probe's leaf order, and an outer join's probe is the padded
  table (`shape_join`), not the kept table the order came from; a backward scan was read forward;
- `morsel_rows` sorted every statement's groups ascending.

One-table scans' rows were guarded (`scanned_rowids` takes forward scans only); joins' rows and every
statement's groups were not. The column copy hands its groups over through `morsel_rows` too, so a
COLUMNAR table gave them ascending with parallel mode off.

**Fix.** `SelectPlan::loop_order` keeps the ORDER BY the loops give, and a join's workers sort by it as
by one the planner kept. `MorselAggregateData::group_order` carries the GROUP BY's directions and NULLs'
places, and the groups are sorted by it as the serial sorter compares keys (`cmp_in_column`).

| Check (Mac Studio d1323898, macOS arm64, dev profile) | main fab12d7e | with the fix |
|---|---|---|
| `t_qt_6_36` (new; the test alone, commit 3f76bfaa) | FAILED, 2.51 s: "SELECT d.c, count(*), min(f.b) FROM f LEFT JOIN d ON f.a = d.a GROUP BY d.c ORDER BY d.c NULLS LAST on 1 cores, ByCost, Padded: the order" | passes, 20.58 s |
| shapes probe: 393 join statements (and the nine of t_qt_6_35), 2 join methods, 2 outer orientations, 3 searched-link settings, stated 1, 3 and 8 cores and measured costs at 1 and 4, each answer judged in row order by SQLite 3.51.0 | 20,696 answers of 393 statements: workers 7,930, 1,020 wrong (all the same rows in another order); serial loop 12,766, 0 wrong | 17,911 answers of 393 statements (every one planned by cost, some under HashJoinsOnly): workers 6,760, 0 wrong; serial loop 11,151, 0 wrong |
| `t_qt_6_36_a_copys_groups_come_out_in_the_order_stated` (new; main's execute.rs put back for the red) | FAILED: `SELECT s, count(*), max(v) FROM t GROUP BY s ORDER BY s DESC` gave NULL, k0, k1, k10, ... from the copy and k9, k8, k7, ... from the rows | passes, 0.63 s, and the copy answered |
| `translate::` and `versioned::columns` suites (8 threads, 107 s) | `t_pl_8_13` red with the same message (85 s) | 335 passed, 1 failed (`t_pl_8_13`, main's own) |
| `morsel` suite (`--skip t_qt_6_18`, 6 threads) | `t_qt_6_35` red | 48 passed, 1 failed (`t_qt_6_35`, main's own), 4 ignored, 31.5 s |
| clippy `-p amphora-engine --all-targets -D warnings` | | exit 0 twice (a7c00cdd, 7bfaf2ac), the log shows `Compiling amphora-engine` |
| `cargo fmt --all --check` | | clean |
| engine production lints, counted with the ratchet's lints and features | 10,619 | 10,619, each lint the same |

## t_qt_6_35

- **Red since its first build.** `af119522` added it, "not compiled when written (kiln down)" (its own
  words; not built here). `f95c82ba` (2026-11-22, lane qs6-18's compile fixes) builds, and it fails
  there with today's message and today's plan for the statement; so it does at 4fd15364, the landing
  on main. It has no green commit to bisect to. T19 (a34ec88b) and T20 and T21 (e23ab4b6) came a day later and have no part in it.
- **Why no link.** `shape_join` takes an inner join only when at least one table is read by a hash
  join (executor.md, lane qs6-10: "at least one of them a hash join"). The planner gave none to 4 of
  the 9 statements (1, 3, 5 and 9): 1 and 5 an automatic index on d, 3 and 9 two rowid ranges.
- **Why no hash join (the planner's own trace, statement 1).** With no ANALYZE each table counts as
  1,000,000 rows, so `f.id > 2000` is 400,000 rows. The hash join is costed at 91,407,663, below the
  automatic index loop's 91,900,701, and is refused: a build read by a range is materialized first,
  and 400,000 is past `MAX_MATERIALIZED_BUILD_ROWS` (200,000, "a safety limit, not a cost tuning
  parameter", join.rs). `HashJoinsOnly` forces the cost but not the limit, so the sibling tests' way
  (776d28b2, 232a8f7e) doesn't help here. With ANALYZE the same statement estimates 978.5 rows, the
  hash join costs 188.1 against 305.6, and all four plan hash joins by cost.
- **Answers.** At the test's shape every one of the 9 statements, under 5 join-method settings, 2 outer
  orientations, 3 searched-link settings and 1, 2, 3, 4 and 8 stated cores, equals SQLite 3.54.0's:
  1,350 parallel and 45 serial answers, 0 wrong (adacachy2, before the window).
- **What the test should say.** The design takes an inner join only with a hash join in it
  (executor.md, lane qs6-10) and doesn't promise the planner makes one without statistics, while
  T-QT.6.35 asks for a link in each statement. Run on 7bfaf2ac, assertions unchanged:
  - ANALYZE before the statements: still red, at statement 2 (by cost it takes nested loops then).
  - ANALYZE and `HashJoinsOnly`: passes, 2.71 s, all 9 statements at every core count. That is what
    776d28b2 and 232a8f7e did for T-QT.6.29 to 6.32 (`HashJoinsOnly`), with the statistics the limit
    above needs. Not landed: the coordinator's to decide.
  - The planner's choice isn't the test's to change. The limit refusing the cheaper plan on a fallback
    estimate (400,000 rows guessed for 1,000) is the planner's lane's question.

## Suites after the landing (7bfaf2ac, the landed code)

`--test hash_group_by` 5 passed, `--test join_methods` 4 passed (1 ignored), `--test nested_loops` 7
passed, `--test planner` 13 passed and 1 failed
(`a_whole_table_group_by_scans_and_sorts_where_lookups_would_read_pages`, planner.rs:182: "800 pages",
a scan by the index where the test expects the table's), which fails on fab12d7e too, before this
landing. `--test top_n` did not run: cargo stopped at planner's failure.
