# Lane C4 (tests), 2026-11-24 06:44:49Z to 07:49Z: t_qt_6_35 green, and which change made planner.rs:182 red

The hour started at the first command, 06:44:49Z. The coordinator set its end at 07:49Z.

## Verdict, losses first

- **t_qt_6_35 passes with its assertions as they were, and covers less than it says.** Its comment and
  T-QT.6.35 (docs/requirements.toml) promise inner joins "whose probe, a link or both the planner
  reads by a rowid range". In its 45 parallel runs no probe has a range, only links do. An engine
  fault that drops a ranged probe's leaves passes it (below). A hash join's probe holds no range
  (`scanned_rowids` takes a search or a whole scan, and the probe's operation is the hash join), so by the
  code no two-table statement under `HashJoinsOnly` can give one. No assertion passes that should not, but the
  text promises what no statement of it exercises. Not changed here: the statements are the test's.
- **planner.rs:182 is not fixed.** Its first cause is the planner's, below, and it goes to T22.
- **No macOS run.** No Mac had room: three runs (cpus 4 to 8) asked 06:48Z to 06:54Z were still queued at
  07:33Z, when I stopped them. C1's red was
  on macOS. All of this is adacachy2 (Linux x86_64), dev profile, load 42 to 75.
- **I stopped the bisect to make room and lost its place** (07:12Z to 07:17Z): the machine had no idle
  core by then. Over two dozen asks were refused. The directed pair below is in its place.
- **The two plans were timed in the dev profile only, on a loaded machine, in one process.** Release was
  not run.
- One run asked 2 cores and ran clippy (2 jobs) beside the tests (2 threads).

## t_qt_6_35

Setup, as the coordinator decided: `ANALYZE`, then `HashJoinsOnly`, after the tables are filled, as
776d28b2 and 232a8f7e did for T-QT.6.29 to 6.32. The reason is executor.md's lane qs6-10: the parallel
path takes an inner join only with at least one hash join in it. Assertions unchanged, 171e902c.

| Run (adacachy2, dev profile) | Result |
|---|---|
| the test alone, 171e902c (r721) | passes, 4.29 s |
| morsel suite without t_qt_6_18 (r721) | 49 passed, 0 failed, 4 ignored, 69.6 s |
| clippy `-p amphora-engine --all-targets -D warnings` (r902) | exit 0, 202 s, the log shows amphora-engine checked |
| check-production (r902) | exit 0, 115 s: 10,617 left in the engine, as the ratchet holds |
| `cargo fmt --all --check` (local, and again on the merge) | exit 0 |
| the merged tree | see "The merged tree" |

Negative controls (r915, a throwaway copy with the test's body repeated under other settings; each must
fail):

| Control | Result |
|---|---|
| join forced serial (parallel mode off) | FAILED: "the hook built no link", statement 1 |
| hash join removed, `ByCost` (ANALYZE stands) | FAILED: "the hook built no link", statement 2 |
| hash join removed, `MergeJoinsOnly` | FAILED: "the hook built no link", statement 1 |
| no ANALYZE (`HashJoinsOnly` stands) | FAILED: "the hook built no link", statement 1 |
| neither (main's setup) | FAILED: "the hook built no link", statement 1 |
| a fault: links' lower bound moved up 200 rowids | FAILED: statement 1, 10,028 rows against 11,691 |
| **a fault: the probe's lower bound moved up 200 rowids** | **passes** |

The ranges the hook was given, of 9 statements at 5 core counts, 95 reads: the probe's, 45 times, none.
A link's, 30 times, one each of `>= 2900`, `>= 2000`, `<= 50`, `<= 400`, `<= 300` and `<= 100`. A link's
with none, 20 times. The links' ranges are cut by statements 1, 2, 3, 4, 5 and 9. The outer joins (6, 7
and 8) read theirs whole by design. `e.id > 200` (4), `d.id > 550` (3), `f.id > 100` (9) and `f.id`'s
range (2) are never given to the hook as ranges.

## planner.rs:182, `a_whole_table_group_by_scans_and_sorts_where_lookups_would_read_pages`

- **It is red most of the time, not always.** On main fab12d7e (before C1's landing) 5 of 6 processes fail
  and 1 passes (r724). At 171e902c all 14 of `--test planner` passed once (r904). perf-coldstart's record
  (r89) has it failing 2 of 3 and 3 of 3 runs on adacachy2 under load, and d2f9c9b0 1 run in 7 with the
  fitted-cost switch on. C1 saw it red on a Mac, at 7bfaf2ac and at fab12d7e.
- **What it measures.** `SELECT grp, COUNT(DISTINCT owner) FROM items GROUP BY grp` with a 200-page cache
  and 800 pages of table. Decision 5 (planner.md) says the lookups the index order needs each read a page,
  so it scans and sorts.
- **The formulas are not the cause.** `cost.rs` and `order.rs` are byte for byte what they were at
  190dc39e (PL.7's landing, the test's last change), and `choose_best_btree_candidate` is untouched. With
  the static weights (learned weights off) the planner charges the scan and sort 3,649 pages and the index
  order 61,399 at every commit tried, and the test is green at every one of them.
- **The weights are.** The plan turns on one number, the sort's, in page reads. It is right while
  `1,303,000 x sort` stays under the index order's 60,000 to 68,000 pages, 0.046 to 0.052 a unit (rows
  times log2 rows).
- **First red commit: 18daab1e** (2026-11-22 02:10 -0800, PL.8, T-PL.8.6, lanes pl8-14 and pl8-15,
  `learned_costs.rs`). Its one parent, d27d8df4, is green 6 of 6. It is red 6 of 6, minutes apart on one
  machine. 98c5a931 (pl8-13's significance test) is in that parent, so it is not the cause. T13 and T15 (the
  page-miss share and the page read's timing) come later and are not needed to explain the red. They were
  not measured apart.

Each row is 6 processes, each with a calibration of its own, at one commit. "Sort weight" is the learned
weight in row visits a unit. "Cost ratio" is the plan cost of scan and sort over index order, the median:
above 1 the planner takes the index order and the test is red. Load 48 to 75 while it ran.

| Commit | green | sort weight | cost ratio |
|---|---|---|---|
| 190dc39e (2026-11-21, PL.7's landing, the test's last change) | 5 of 6 (its red is the last assertion, planner.rs:189, at a sort weight of 0.35) | 0.35 to 4.3 | 0.20 |
| d27d8df4 (18daab1e's parent) | 6 of 6 | 2.2 to 6.9 | 0.29 |
| **18daab1e** | **0 of 6** | 13.0 to 17.1 | 1.52 |
| fab12d7e (main before C1) | 1 of 6 | 12.6 to 21.9 | 2.10 |

- **What 18daab1e decides.** "The first measured batch replaces calibration's rate", and a later batch
  moves it only past the spread between batches. Before it the rate followed the last one to two epochs
  (190dc39e), or moved on significance (98c5a931). With few batches Student's t is large, which
  decision pl8-15 (planner.md) lists as not yet built: "the weights move slowly until batches gather". The
  sort's price here is the first two sorts the test makes, CREATE INDEX's, and the statement's own are
  judged by it.
- **The mechanism, traced (r988, fab12d7e, load 67, one process).** Each in-memory sort the test's build
  reports, in nanoseconds a unit, with the rate that stands after it:

  | sort | took | the rate then stands at |
  |---|---|---|
  | before any | | 27.8 (calibration) |
  | CREATE INDEX items_name (TEXT) | 98.3 | 27.8 |
  | CREATE INDEX items_grp_created | 135.8 | **117.0** (the mean of the two, the first batch) |
  | CREATE INDEX items_owner | 43.0 | 117.0 |
  | CREATE INDEX owners_region (400 rows) | 7,515.9 | 117.0 |
  | the statement's own, 10 times | 14.5 to 28.6 | 117.0 each time |

  The planner charges 117.0 ns a unit for a sort that takes 14.5 to 28.6. That is 4.1 to 8.1 times too
  much, and calibration's 27.8 was close. Batch means move a rate only past the spread between batches,
  and after the first batch there isn't one.
- **The plan it picks is the slow one (same run).** With the 200-page cache, the same statement forced each
  way, 5 times: scan and sort 346 to 465 ms, index order 1,923 to 2,278 ms. The plan made at a fresh
  open, at the anchored page read (1,249 ns), is the index order. Once the database had read pages of its
  own, the page read's rate stood at 21,600 to 28,800 ns, and the statement planned again was scan and sort
  (326 to 385 ms). Dev profile only. Release was not run.
- **The page read's price swings too.** It stands at the open's anchor (1,249 ns here) until the database
  has read pages of its own, and the row's weight in page reads varied 18 times between the 6 processes at
  fab12d7e (0.00073 to 0.0133). Not separated from the sort's here.
- **The expectation didn't change; the planner's price did.** The test's plan at a 200-page cache has been
  "scans and sorts" since 2026-10-23 (4acff7df), as planner.md decisions 5 and 7 promise in those words
  (the Evidence paragraphs). So the test matches its promise, and nothing here is loosened or changed.
- **Owner.** Decision pl8-15 and decision 5 are the planner lane's (T22). The evidence is this section, and
  adacachy2's `~/.cache/amphora-perf/tests-c4-bisect/*.out` (each run's traced weights and candidate costs).

## What ran

- r721 (the test and the morsel suite), r902 (clippy and check-production), r904 (`--test top_n`,
  `--test planner`), r915 (controls), r724 and r903 (the planner's test at fixed commits), all on
  adacachy2, dev profile, loads 42 to 75.
- `--test top_n` (r904): 3 passed, 0 failed, 3.5 s:
  `a_limit_bound_as_a_parameter_keeps_the_rows_it_names`,
  `only_an_ordering_with_a_limit_tells_its_sorter_how_many_rows`,
  `rows_ordered_with_a_limit_are_the_first_rows_of_the_whole_ordering`. C1's never ran, cargo stopped at
  planner's failure.
- `--test planner` (r904): 14 passed, 0 failed, 96 s.

## The merged tree

main moved twice while this ran (lanes C5 and PG18, then B23). Both merges were re-run on adacachy2:

| Tree | t_qt_6_35 | morsel suite | clippy -p amphora-engine --all-targets | check-production |
|---|---|---|---|---|
| 1eaff6c8 (60993be4 merged, r950) | passes, 3.60 s | whole, t_qt_6_18 with it: 51 passed, 0 failed, 4 ignored, 95.9 s | exit 0, 44 s | |
| 135ed91c (b53a29b1 merged, r979), on main | passes, 4.85 s | | exit 0, 62 s | exit 0, 93 s: 10,617 left in the engine, as the ratchet holds |

Pushed to main as 135ed91c at 07:32Z. The production code there is main's own: the change is six lines in
a `cfg(test)` file.

## Decisions for the coordinator

1. T-QT.6.35's text. Either add a statement whose probe the planner reads by a range (a joined table
   found by a seek, with `set_parallel_searched_links`, gives one: the hook's probe is then the outer
   loop's table), or take "a probe" out of the text and the test's comment. Nothing else in the tree runs
   a ranged probe in a join: T-QT.6.27's statements have no range.
2. planner.rs:182 (T22). Either pin the test's cost weights (`Connection::pin_cost_weights`, lane T9) so it
   holds decision 5's page count and not a dev build's sort price, or the planner decides the sort's weight
   some other way before enough batches gather (decision pl8-15's "Not yet built").
