# The planner: the plan with the least work

Amphora's engine plans a query with the cost model it came with (`crates/engine/translate/optimizer`,
its `OPTIMIZER.md`): a dynamic-programming search over join orders, each table's access path costed
in page reads, seeks and rows. Each decision here changes that model where Amphora's bench suite
(`bench --suite`, AC-T1.2.1) found it choosing a plan that does far more work than one it had
considered, recorded with its sources, its evidence and what would falsify it (CLAUDE.md §5).
`crates/engine/tests/planner.rs` holds each shape the suite found, on a smaller copy of its tables.

## 1. A closed range is half a percent of its table, whether it seeks or filters

**Decision.** A range bounded on both sides on one column whose bounds no statistics place
(`id BETWEEN ? AND ?`, `x > ? AND x < ?`) keeps 0.005 of the rows (`sel_closed_range`), the same
estimate whether the range drives a seek (`cost.rs`, `range_selectivity`) or filters rows
(`constraints_selectivity`, which the join search's output estimates use), and a `BETWEEN` in a
partial index's condition takes it too. A bound alone keeps 0.4, as before.

**Why.** The two estimates disagreed by five times: a seek's range kept 0.4 × 0.4 = 16% of the
table, the same range as a filter 0.4 × 0.4 × 0.2 = 3.2%. So the planner avoided seeking by a range
it took for large, then credited the same range for cutting rows once it was a filter. For the
suite's `join_pk` (the `items` rows of 20 consecutive ids, joined to their owners) it scanned all
4,000 owners and searched `items_owner` for each (estimated cost 14,334, against about 71,000 for
reading the range first, which it took for 32,000 rows): 4,000 index searches and 200,000 index rows
for 20 results. With 0.005 the range is 1,000 rows, reading it first costs about 2,250, and the plan
is SQLite's: the range, then each owner by key.

**Sources.** PostgreSQL's `DEFAULT_RANGE_INEQ_SEL`, 0.005 ("default selectivity estimate for range
inequalities A > b AND A < c", `src/include/utils/selfuncs.h`), which `clauselist_selectivity`
(`src/backend/optimizer/path/clausesel.c`) takes for such a pair when either bound's estimate is a
default: "no data available --- use a default estimate that is small, but not real small". SQLite's
`whereRangeScanEst` (`src/where.c`) takes a closed range for 1/64 of an index without STAT4 data,
the same reduction for the loop's rows as for its cost.

**Evidence.** Before and after on fedora, 1 client, 200,000 items, at load 7 (a check, not a
recorded run; the recorded one follows the suite's clean rerun): `join_pk` 71 → 94,710 statements a
second (SQLite 68,422), `join_three` 56 → 1,598 (SQLite 2,214). The suite's other statements keep
their plans: of its 50, `--explain` before and after differs in 13, three of them these decisions'
and ten only in how EXPLAIN words a range on the rowid (it said `rowid=?` for every rowid search).

**Falsified by.** A workload whose closed ranges routinely keep a large part of a table (above about
5%), where driving from such a range does more work than the plan the old estimate chose. The cure
then is the bounds' values (a histogram, or planning with the bound parameters), not another
constant.

## 2. The sort is charged to the candidate that needs it, on the rows it sorts

**Decision.** Choosing a table's access path, an ordered candidate and an unordered one compare
with the sort added to the unordered one's cost, whichever was considered first. The sort is costed
on the table's rows after the filters ready at that point, not on the whole table. The rowid
candidate is costed as covering: the table's B-tree holds every column, so a scan in rowid order
reads each page once.

**Why.** The suite's `offset_deep` (`SELECT id, name FROM items ORDER BY id LIMIT 20 OFFSET ?`):
the scan in rowid order, which needs no sort, was charged its pages twice (as an index's leaves and
again as table lookups) and, once chosen, lost to a later unordered candidate, the covering index
`items_name` read whole and then sorted, because the sort was credited only when the ordered
candidate came second. Costing the sort on the whole table would undo the fix elsewhere: a seek
returning 50 rows, charged for sorting 200,000, would lose to an ordered scan of all of them.

**Sources.** SQLite's `whereSortingCost` (`src/where.c`): a plan's sort is costed on the rows that
plan produces (N log N, less with a LIMIT), and `wherePathSolver` adds it to every path that does
not deliver the ORDER BY's order, whichever is found first.

**Evidence.** `offset_deep` 25 → 64 statements a second on the same check (SQLite 128): its plan is
SQLite's, the table in rowid order. The rest is the engine's speed through rows, a separate matter.

**Falsified by.** A query where a candidate the model now prefers for its order does more work in
measurement than the sorted alternative it beat (the model has no notion of LIMIT stopping an
ordered scan early, nor of a sort spilling).
