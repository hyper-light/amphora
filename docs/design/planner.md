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
measurement than the sorted alternative it beat. The model has no notion of a sort spilling. It
has a notion of a LIMIT stopping an ordered scan early only for a one-table read (decisions 16
and 17).

## 3. A GLOB's literal prefix is a range of the index; a BLOB matches no GLOB or LIKE

**Decision.** `col GLOB p`, with `col` a column of TEXT affinity in BINARY order and `p` a literal
or a bound parameter, implies `col >= glob_lower_bound(p)` and `col < glob_upper_bound(p)`
(`constraints.rs`, `glob_bound_constraints`): the pattern's literal prefix, up to its first `*`,
`?` or `[`, and that prefix with its last ASCII character advanced (what follows it dropped, so the
bytes stay UTF-8; an empty BLOB, after all text, when nothing is left). Both are computed when the
statement runs, so a prepared statement's parameter serves. An index on the column seeks the range;
the GLOB still filters, as the bounds are implied (`Constraint::implied`: using them consumes
nothing, and as filters they count for nothing beside the GLOB). A BLOB matches neither GLOB nor
LIKE: so the range holds every match, since a TEXT-affinity column holds only text, BLOBs and
NULLs.

**Why.** The suite's `like_prefix` (a prefix of an indexed name, 50 rows) read the whole index:
Amphora's LIKE and GLOB never used one. Its LIKE ignored case where PostgreSQL's and MySQL's with
binary collations heed it, so the suite asked SQLite's dialect for more than the others: it now asks
it for GLOB, the same match (`bench/src/suite.rs`).

**Sources.** SQLite's LIKE optimization ("The SQLite Query Optimizer Overview", section "The LIKE
Optimization"): the conditions above (the column's TEXT affinity, BINARY collation for GLOB, a
literal or a bound parameter as the pattern) and the bounds, the prefix's last character
incremented. SQLite's compile-time options ("Recommended Compile-time Options",
`SQLITE_LIKE_DOESNT_MATCH_BLOBS`): LIKE and GLOB return false for a BLOB, recommended as it lets the
LIKE optimization run without a second pass over BLOBs, which SQLite makes otherwise.

**Evidence.** `crates/engine/tests/glob_range.rs`: the plan seeks `t_name (name>=? AND name<?)` for
a parameter and for a literal, and not for NOT GLOB; for 36 patterns (wildcards first, last and in
classes, the last ASCII character `~` and DEL, accents and other scripts, digits, the empty
pattern) a seek reads exactly the rows a scan of an unindexed copy reads, and never a NULL or a
BLOB; the bounds of each shape of pattern. With the upper bound left as the prefix itself (a
mutant) the scan comparison and the bounds' test fail.

**Falsified by.** A row a GLOB matches that its seek misses; a statement that relied on a BLOB
matching a LIKE or a GLOB.

## 4. A constraint on an index's second column skip-scans it

**Decision.** Where no constraint seeks an index's first column but one seeks its second, and
ANALYZE counted at least 18 rows to each value of the first, the planner costs a skip-scan too
(`access_method.rs`, `skip_scan_refs`): a seek for each distinct value of the first column, with
the constraint's range within it. The first column must be a plain ascending one with its NULLs
first and the index not partial; the scan reads forwards. Its cost is a seek for each value,
counted 1.375 times over, each reading its value's share of the rows the constraints keep
(`cost.rs`, `estimate_skip_scan_cost`). The bytecode rewinds the index, reads the first entry's
first column into the key's first register and seeks within that value (`seek.rs`,
`emit_skip_scan_start`); where the range ends, or a seek within a value finds nothing, a `SeekGT`
on that one column finds the next value, and the key is built again for it. A NULL is a value like
any other: the skipped column's key component matches NULL, and a one-sided range keeps its NULL
boundary within each value as within an equality prefix. EXPLAIN words it as SQLite does:
`ANY(grp) AND created>=? AND created<=?`.

**Why.** The suite's `count_range` (`COUNT(*)` of a 10,000-row range of `created`, which only
`items_grp_created (grp, created)` holds, second) scanned the whole table: 160 statements a second
at 1 client against SQLite's 4,553, the widest gap to an engine of its kind in the suite. SQLite's
plan for it is this skip-scan.

**Sources.** SQLite's `whereLoopAddBtreeIndex` (`src/where.c`): a skip-scan is tried where no WHERE
term constrains an index's left-most column and ANALYZE (`sqlite_stat1`) found that column's values
repeated 18 times or more on average ("scanning 17 rows is almost always quicker than an index
seek"), its iterations given a 1.375 fudge factor; its code (`codeAllEqualityTerms`,
`sqlite3WhereEnd`): a rewind, the skipped column read into the key, `OP_SeekGT` on it for the next
value. "The SQLite Query Optimizer Overview", section "The Skip-Scan Optimization": the same
conditions in words, ANALYZE among them.

**Evidence.** `crates/engine/tests/skip_scan.rs`: after ANALYZE the plan is `SEARCH t USING COVERING
INDEX t_idx (ANY(g) AND c>=? AND c<=?)`; none before ANALYZE, with a first column all but unique, or
with the first column constrained. Ten shapes of filter (ranges open and closed, an equality, `IS
NULL`, an empty range, a residual on the first column) on four tables (an integer first column, a
case-blind text one, a descending second column, a third column after it), each read five ways
(rows, aggregates, ordered by the index, ordered by the second column, under a LIMIT) and inside a
join, read exactly what the same statements read from an unindexed copy; 41 of them skip-scan.
UPDATE and DELETE through a skip-scan (of a column outside the index, of the scanned column moved
past its range and within it, of rows deleted) leave the rows the same writes leave in the unindexed
copy, the index agreeing with the table after. Counting about 530 rows of 30,000 runs fewer than
2,700 instructions, where a scan runs some for every row. Four mutants each fail them: the skipped
column not matching NULL, a seek that finds nothing in one value ending the scan, a range's end
ending it, no NULL boundary within a value. Release binaries on fedora (200,000 items, best of 3 in
turns; load 2–7 from other work, so checks, not recorded runs): `count_range` 165 → 2,476 statements
a second at 1 client (p99 7.4 → 0.47 ms) and 2,644 → 32,892 at 32 (p99 14.8 → 1.5 ms);
`range_secondary_100` and `order_by_index`, seeks of the same index, unchanged.

**Falsified by.** A row a skip-scan misses or adds; a statement a skip-scan makes slower than the
scan it replaced (a first column whose values hold few rows each, where ANALYZE's average misleads).

## 5. A table lookup reads a page unless the connection's cache holds it

**Decision.** Through an index that lacks columns a statement needs, each row is a seek of the
table of its own. Its cost is the table pages those lookups read, by Mackert and Lohman's
approximation for a finite LRU buffer (`cost.rs`, `table_pages_fetched`), plus a seek's CPU for each
row. The buffer is what a read finds without reading the file: the connection's page cache and the
pages its database shares (`Pager::cached_page_capacity`), taken at prepare, so a statement
prepared after `PRAGMA cache_size` changes is planned for the new cache. The model charged instead
the table's pages in the rows' proportion, as if every row it looked up shared a page with the
next.

**Why.** The suite's `table_group_by` (`GROUP BY grp` over 200,000 rows) walked
`items_grp_created` for its order and looked every row up in the table at random, 4 statements a
second. Timed on fedora on a copy of its table (a probe of the engine, release): that plan took 257
ms with the connection's 2,000-page cache and 90 ms with the table all cached; a scan of the table
and a sort took 75 and 36 ms. Mackert and Lohman count about 188,000 pages read for 200,000
lookups among 33,000 pages with 2,000 cached; the model counted the table's 33,000 pages once.

**Sources.** Mackert and Lohman, "Index Scans Using a Finite LRU Buffer: A Validated I/O Model",
ACM TODS 14(3), 1989: the pages a scan of `Ns` rows at random among `T` pages reads with `b` cached.
PostgreSQL's `index_pages_fetched` (`src/backend/optimizer/path/costsize.c`): the same formula, its
three cases, used for every index scan's table reads (with `effective_cache_size` for `b`: here
only what spares a read of the file counts, since a page the OS caches still costs one). SQLite's
`whereLoopAddBtreeIndex` (`src/where.c`): a non-covering index adds the cost of a search of the
table for each row it returns.

**Evidence.** `cost.rs`, `table_pages_fetched_follows_mackert_and_lohman`: the three cases.
`crates/engine/tests/planner.rs`, `a_whole_table_group_by_scans_and_sorts_where_lookups_would_read_pages`:
with a 200-page cache the GROUP BY scans and sorts; with the table cached it keeps the index's
order. Of the suite's 50 statements, `--explain` before and after: only `table_group_by` changes
plan. Release binaries on fedora (200,000 items, best of 2 in turns, each run started at load under
2; checks, not recorded runs): `table_group_by` 4.0 → 14.2 statements a second at 1 client (p99 270
→ 74 ms) and 35.2 → 167.5 at 32 (p99 1,163 → 238 ms); `group_by_range` and `point_secondary`
unchanged.

**Falsified by.** A statement the model now sends to a plan that reads more. It knows a table's
pages only as its rows over 50 (`rows_per_table_page`), not their real size: a table of wide rows
has many more, which makes its scans look cheaper than they are.

## 6. Many searches of an index the cache holds read its pages once

**Decision.** The searches of a nested loop's inner side, of an index whose pages all fit in the
connection's cache, are charged each page once at most: its leaf pages as Mackert and Lohman count
them for that many searches, its inner pages once (`estimate_index_cost`). One pass, and an index
bigger than the cache, keep the model's charges for each search. The model charged every search
but the first the pages below the root, even of a table of eight pages that never leaves the cache,
while it charged an index built on a subquery's rows CPU alone for each probe: so it would rather
build one than search a small table by its key.

**Why.** The suite's `cte` (`WITH t AS (SELECT owner, SUM(price) ... WHERE grp = ? GROUP BY
owner) SELECT o.name, t.s FROM t JOIN owners o ON o.id = t.owner ORDER BY t.s DESC LIMIT 5`) scanned
all 4,000 owners and probed an index built on t's rows for each, where t's 1,650 rows, each looking
its owner up by key, do less. On the planner test's copy, 200 searches of `owners` (eight pages)
were charged 240 pages and 400 probes of the built index 4.

**Sources.** Mackert and Lohman, as in decision 5. PostgreSQL applies `index_pages_fetched` to an
index's own pages over a scan's repetitions as to its table's (`genericcostestimate`,
`src/backend/utils/adt/selfuncs.c`, with `loop_count`): an index read many times costs the pages
those reads fetch, not each read's pages.

**Evidence.** `cost.rs`, `repeated_searches_read_no_more_than_the_cache_cannot_hold`: 200 searches
of a 1,000-leaf index the cache holds cost its leaves as Mackert and Lohman count them and its inner
pages once; with a one-page cache, every search's pages as before (the model's other tests run so).
`crates/engine/tests/planner.rs`, `a_grouped_cte_drives_lookups_of_the_owners_it_names`: the CTE,
and the same subquery in FROM, drive lookups of `owners` by key, and return the rows of the same
join without a subquery. Of the suite's 50 statements, `--explain` before and after: only `cte`
changes plan. Release binaries on fedora (200,000 items, best of 3 in turns, each run started at
load under 2; checks, not recorded runs): `cte` 218 → 324 statements a second at 1 client and
2,372 → 2,904 at 32; `order_by_sort` and `group_by_having`, which no plan change touches, about 6%
faster at 1 client from decision 21 of `store.md`, built into the second binary only.

**Falsified by.** A nested loop the model now prefers that reads more than the plan it replaced;
an index the cache doesn't hold although the model, counting its pages from its rows, says it does.

## 7. A GROUP BY that groups in a hash table costs a probe a row, not a sort

**Decision.** Where the planner weighs reading a GROUP BY's rows in its order (an index that
supplies it) against reading them out of it, what reading them out of order costs depends on how
the GROUP BY would then group. One that would group in a hash table (`docs/design/executor.md`
decision 4: aggregates of a few values, no DISTINCT, no window; `group_by::groups_may_hash`) costs
a probe a row and a sort of its groups as the table drains (`cost::order_cost`), when ANALYZE
counts its groups (the distinct values of an index of its table those columns lead) and they fit
the table's budget as it charges them (`group_table::groups_in_budget`, 64 MiB). Otherwise it costs
a sort of the rows, as before. The same cost stands where the plan pays for the GROUP BY itself
(`required_group_sort_and_read_cost`), which then reads back its groups, not its rows.

**Why.** The executor stopped sorting such GROUP BYs, but the planner still priced not supplying
their order as a sort of every row (a sort's `n log n`), so an index that supplied it saved, by
the model, a sort that would never have run. With the cache large (the process's page pool,
decision 26 of store.md, or a large `cache_size`), decision 5 priced the index's lookups at a page
each once and a seek's CPU, and the model chose to walk `items_grp_created` and look up every row:
the suite's `table_group_by` at ten million rows, `SCAN items USING INDEX items_grp_created`, 87% of
its time in seeks (`seek_deferred_row`, profile of 2026-10-23), 16.7 s a statement at one client
and 50 s at 32, where the scan and the hash table took 4 to 5 s and 15 s (`ab-v4-10m.jsonl`).
Decision 5's own measurement already had the index's order losing with the table all cached (90 ms
against 36 ms for a scan and a sort, 200,000 rows).

**Sources.** SQLite's planner charges a GROUP BY or ORDER BY a sorter only on a path that leaves
its order unmet (`whereSortingCost`, `src/where.c`); PostgreSQL costs hashed and sorted
grouping apart and keeps the cheaper (`create_agg_path` with `AGG_HASHED` against `AGG_SORTED`,
`cost_agg` in `src/backend/optimizer/path/costsize.c`), a hash aggregate's cost a probe a row
(`cpu_operator_cost` per grouping column a row).

**Evidence.** `crates/engine/tests/planner.rs`,
`a_whole_table_group_by_scans_and_sorts_where_lookups_would_read_pages`: the hashed GROUP BY scans
and hashes its 100 groups with a 200-page cache and with the table all cached; a COUNT(DISTINCT)
GROUP BY, which sorts, scans and sorts with the small cache and takes the index's order with the
table cached, as decision 5 has it. `cost.rs`,
`a_hashed_group_by_costs_a_probe_a_row_and_a_sort_of_its_groups`.

**Falsified by.** A GROUP BY this sends to a scan and a hash table that an index's order would have
answered faster (groups ANALYZE undercounts, which overflow the table into the sorter).

## 8. Without statistics, a primary key's column is unique only as the whole key

**Decision.** An equality is costed as a unique lookup (one row in the table) only on a rowid alias
or on the column of a primary key that has that one column (`constraints.rs`,
`estimate_selectivity`). A column of a composite primary key is costed as any indexed column:
ANALYZE's rows per value when it has counted them, the indexed-column selectivity when it hasn't.

**Why.** Every column a table-level `PRIMARY KEY (a, b, c)` lists carries the primary-key flag
(`schema.rs`), and the estimate took the flag alone as uniqueness. Without statistics, on kiln's
versions table (`PRIMARY KEY (family, key, rev)`, an index `versions_by_rev (family, rev, n)`),
`family = ?` alone came to one row in a million, `family = ? AND key = ?` to a millionth of a row,
and every seek of either index floored to one row at the same cost: the choice fell to a tie-break.
Kiln's newest-version read walked a family's every version by revision, and its change feed sorted
a family's every version, where SQLite 3.54.0 seeks each on the index it needs (reported by Sends
T3 for CP-T1.5, kiln plan 01a1713cc0137649b9409364d81404d7).

**Sources.** Without `sqlite_stat1`, SQLite estimates an index by fixed defaults
(`sqlite3DefaultRowEst`, `src/build.c`): ten rows a value of its first column, nine of its first
two, then eight, seven, six and five, and one row only for a unique index's whole key, so an index
that matches more equality columns always gives fewer rows; it divides a seek's estimate by four a
range bound (`whereRangeAdjust`, `src/where.c`).

**Evidence.** `crates/engine/tests/records_plans.rs`:
`kilns_reads_are_planned_without_statistics_as_sqlite_plans_them`, twelve forms of kiln's ten reads
(eight as kiln words them, four plainly), failed before the change (kiln's newest-version read
planned on `versions_by_rev` with a sorter) and plans all twelve as SQLite does with it;
`more_equality_columns_and_the_order_a_limit_reads_in_win_without_statistics` and
`indexed_by_uses_the_named_index_or_refuses` give SQLite's plans and SQLite's refusal. With the
whole key's rule the estimates no longer floor for these shapes, so SQLite's absolute defaults were
not needed: two against three equality columns already chose the three-column index.
CERT is unchanged: the oracles' 87 CERT findings of cause AG fail before and after alike, and a
CERT run of one seed (20261025, 300 s) found the same 29 findings with each build over the 2,007
databases both reached, every one failing with both. The suite's 50 statements are planned as
before (`--suite --explain`): their tables are keyed by rowid aliases, unique lookups either way.

**Falsified by.** A composite key's equality the planner now costs above its rows, so that it
passes over the key's index for one that reads more (a key whose first column is nearly unique, on
a table no ANALYZE has counted).

## 9. A correlated subquery that runs in the output is priced at the output's rows

**Decision.** A correlated subquery that an aggregate's or a GROUP BY's output reads (its phase
`UngroupedAggregateOutput` or `GroupedOutput`) is planned for, and costed at, the query's estimated
output rows when they are fewer than the rows of the loop where its outer columns are read
(`find_select_plan_form`, `subquery_calls_where_it_runs`); any other correlated subquery keeps its
loop's rows.

**Why.** Since d4efe6b such a subquery runs at each read in the output (`executor.md`, decision 6):
once a group, or once for an ungrouped aggregate. The planner still counted its calls as its loop's
rows (`count_subquery_calls_for_plan`), so the correlated form cost a call for every row grouped,
and the unnested group-first form (`unnest.rs`), which reads every key of the inner table, won the
cost comparison (`optimize_select_plan_with_cache`). The suite's `group_correlated`, at 200,000
items, scanned all 200,000 entries of `items_grp_created` (4.5 ms a statement at 1 client on
adas-mac-studio-2) where its one group's 1,972 entries take 0.03 ms.

**Sources.** SQLite reevaluates a correlated subquery each time its result is required
(`lang_expr.html`, section 12) and evaluates an aggregate query's output from aggregate columns once
an output row (`TK_AGG_COLUMN`, `analyzeAggregate`): once a group. Cost-based query transformation
applies a rewrite only when the physical optimizer costs it below the original (Ahmed et al.,
PVLDB 17(12), 2024, section 1), so a wrong cost for the original picks the wrong form. Neumann and
Kemper (BTW 2015, sections 3.1 and 3.3) evaluate the dependent side once per distinct outer binding
and leave dropping that restriction, which can make larger intermediate results, to the optimizer:
the group-first form drops it.

**Evidence.** `crates/engine/tests/planner.rs`:
`a_correlated_subquery_a_grouped_output_reads_is_planned_once_a_group`, on the suite's tables with
its 4,000 owners (200 a region) and a fiftieth of its items, failed before the change: the grouped
form unnested into a scan of every item (`SEARCH scalar_subquery_t2 USING INDEX
ephemeral_subquery_t2 (correlation_key_0=?) LEFT-JOIN`, `SCAN items AS i USING COVERING INDEX
items_grp_created`). With the change it plans the grouped and the ungrouped form correlated, a
search of the one group's items, and their counts are the ones read directly.
`a_subquery_an_output_reads_is_priced_once_an_output_row` prices each phase. Of the suite's 52
statements (`--suite --explain`, debug builds of d4f6671 and the change on adas-mac-studio-2), at
its 200,000 items only `group_correlated`'s plan changes, from the unnested scan to the correlated
search of `items_grp_created (grp=?)`; at 20,000 items none does, the correlated form winning
either way. CERT is unchanged: the 87 findings of cause AG fail before and after alike, and a CERT
run of one seed (20261025, 300 s) found the same 76 findings in 248,688 checks with each build.
Release binaries before and after, best of 3 in turns at load under 2 on adas-mac-studio-2
(`bench/records/2026-10-25-adas-mac-studio-2-output-subquery-calls.md`): `group_correlated` 227 →
26,895 statements a second at 1 client (median 4.35 → 0.036 ms) and 1,231 → 225,470 at 32;
`exists_subquery` and `group_correlated_row`, the same programs before and after, within 2%.

**Falsified by.** A grouped query whose correlated form the planner now keeps where the unnested
form reads less (many groups, each reading most of the inner table); a subquery in the output that
runs more often than the output's rows.

## 10. A loop that gives at most one row orders its table and keeps the order before it

**Decision.** A loop that is a complete lookup on a unique key by `=` (every column of a unique
index, or the rowid: `index_access_is_unique_point_lookup`) gives at most one row for each row of
the loops before it (`access_method_is_one_row`). In `plan_satisfies_order_target` the ORDER BY or
GROUP BY columns of its table that come next are taken as ordered, as are any on it that come after
a later loop's, and it keeps the order of the loops before it, so a later loop may continue it. `IS`
doesn't qualify: it can match many NULL keys.

**Why.** The order check ended the order at any loop that supplied none of the next ORDER BY
columns, so a one-row loop whose index doesn't list them sorted its one row: `SELECT v FROM q WHERE
a = ? AND b = ? ORDER BY c LIMIT 10` on q's primary key (a, b), a rowid lookup ordered by another
column, and a join's key lookup after an outer scan in rowid order (`ORDER BY p.id, q.c`), each
with `USE SORTER FOR ORDER BY` where SQLite 3.54.0 has none (found writing decision 8's tests).

**Sources.** SQLite's `wherePathSatisfiesOrderBy` (`where.c`): a one-row loop is "automatically
order-distinct", a unique index whose key columns all match by `==` is order-distinct (`IS` and
`ISNULL` terms can repeat NULL rows), and it marks off "any other ORDER BY terms that reference"
an order-distinct loop, walking the loops only while each is order-distinct. Simmen, Shekita and
Malkemus (SIGMOD 1996, section 5): a key "fully qualified by equality predicates" leaves at most one
record in the stream, under which an order requirement reduces to nothing.

**Evidence.** `crates/engine/tests/records_plans.rs`: `a_one_row_lookup_needs_no_sort_for_its_columns`,
statements on q (`PRIMARY KEY (a, b)`) and p (a rowid table), failed before the change on four:
q's key by `=`, p's rowid by `=`, the join after p's rowid lookup ordered by q.c, and the analyzed
join `SCAN p` then q's key search ordered by p.id, q.c, each with a sorter SQLite doesn't add. With
the change it plans all of them as SQLite does, the lookup first before a scan in rowid order with
its column named before or after the rowid too, and still sorts for `IS` and for the lookup's
column alone after a scan. The suite's 52 statements plan as before at 20,000 and at 200,000 items
(`--suite --explain`, debug builds of 80ce9d7 and the change on adas-mac-studio-2): none orders
after a one-row loop. CERT is unchanged: the 87 findings of cause AG fail before and after alike,
and a CERT run of one seed (20261025, 300 s) found the same 78 findings with each build.

**Falsified by.** A plan that skips a sort its rows need: a loop taken as one-row whose lookup can
return more than one row, or an order carried past a loop whose rows repeat in the ORDER BY's terms.

## 11. Without statistics, a unique key is searched rather than hashed

**Decision.** When a hash join's build table has no ANALYZE statistics (its rows are the
fallback's million) and the join's equality terms would search that table by a unique key from the
probe table (the other join order's index nested loop), the hash join isn't considered
(`build_unique_key_search_without_statistics`, beside the gate that keeps a probe's selective seek).
With statistics, the costs decide as before; a join no unique key can search keeps its hash join.

**Why.** Without statistics every table is taken at 1,000,000 rows, 20,000 pages against a cache
of 2,000, so decisions 5 and 6 charge each search of a key its tree pages and a table-page read: a
million searches cost about 3 million against a hash join's 113,000, and the hash join won on the
guessed size alone. On q (`PRIMARY KEY (a, b)`) and p (a rowid table), empty or 2,000 rows each,
`SELECT p.id, q.v FROM p JOIN q ON q.a = p.x AND q.b = p.y` was `HASH JOIN p, SCAN q` where SQLite
3.54.0 plans `SCAN p`, then a search of q's key for each p row; after ANALYZE Amphora planned it as
SQLite does. A unique key's search reads one row for each outer row whatever the table holds; a
hash join reads the whole build table, whose size without statistics isn't known.

**Sources.** SQLite implements every join as nested loops, building an automatic index where no
index helps, and has no hash join (`optoverview.html`, sections 7 and 14). Leis et al. (PVLDB
9(3), 2015, section 4.1), with estimates often wrong: "Algorithms that seldom offer a large benefit
over more robust algorithms should not be chosen"; they disabled nested-loop joins without an index
but kept index nested-loop joins. Against it: a hash join reads each table once in order where key
searches read pages at random, so for two large tables the cache can't hold it can be faster; the
rule keeps that choice wherever statistics give the sizes.

**Evidence.** `crates/engine/tests/records_plans.rs`:
`a_join_on_a_whole_key_searches_it_without_statistics` failed before the change (`HASH JOIN p,
SCAN q`, empty and filled) and plans SQLite's `SCAN p` and q's key search with it, while its
control, a join on columns no index covers, keeps the hash join. In `join.rs`,
`a_unique_key_join_is_searched_without_statistics` finds the key searched at the fallback's
million rows and a hash join chosen at ANALYZE's million rows each. The suite's 52 statements, run
with ANALYZE, plan as before at 20,000 and at 200,000 items. CERT, whose databases have no
statistics, gains: of the 87 findings of cause AG one now passes (`cert-e749d7942a5b1c1d`) and none
newly fails, and a CERT run of one seed (20261025, 300 s) found 77 findings, all among the parent's
78; the one gone (`cert-3292765d4c2b555e`) estimated an inner join on a one-column key, hashed, at a
million rows, above its own LEFT JOIN.

**Falsified by.** A join without statistics that the key search makes slower than the hash join
would have been (two large tables the cache can't hold, every row joined), measured, not estimated.

## 12. A statement counts its rows at each table access and join when asked, beside the estimates

**Decision.** A connection that asks (`Connection::set_row_counters(true)`) prepares its
statements with a counter at each plan node of every query they hold: one where a query's loops are
entered (its calls), one where each table access's rows enter the next level of the join, or the
query's body after the last (the rows of that join prefix, after every term evaluated there), and
one where a hash join's build puts a row in its table (`vdbe/row_counts.rs`; the counting points in
`translate/main_loop/open.rs`, `body.rs` and `hash.rs`). Each counter is one `CountRows`
instruction adding one to its slot; the slots are a vector the statement sizes at its first count
and zeroes at each reset, so a count allocates nothing and checks its arithmetic. Each counter keeps
the table, the join position and the estimate the planner gave that access, the same
`TablePlanEstimate` that EXPLAIN QUERY PLAN FORMAT=JSON prints, and `Statement::row_counts` reads
them back with the rows counted. A connection that doesn't ask emits no counting instruction.

**Why.** The planner's estimates can't be judged without the rows they estimate: Amphora's join
search ranks orders by each prefix's estimated rows (`prefix_cardinalities`), and the plan's next
steps (docs/research/planner.md) change those estimates; their measure is the q-error at each join
prefix against the rows that ran. Counting where a row enters the next level counts what the
estimate estimates: the rows that passed every term of the prefix, a row an anti-join kept or a
LEFT JOIN null-extended as well as one that matched. Off unless asked, so no statement pays for it.

**Sources.** PostgreSQL's EXPLAIN ANALYZE gives each node its rows and loops (PostgreSQL 17,
`using-explain.html`, section 14.1.2): "In such cases, the loops value reports the total number
of executions of the node, and the actual time and rows values shown are averages per-execution.
This is done to make the numbers comparable with the way that the cost estimates are shown." The
bench divides a prefix's rows by its query's loops for the same reason. SQLite counts each loop of
a statement compiled with them (`c3ref/c_scanstat_est.html`): SQLITE_SCANSTAT_NLOOP is "the total
number of times that the X-th loop has run", SQLITE_SCANSTAT_EST "the query planner's estimate
for the average number of rows output from each iteration of the X-th loop", and the interface "is
only available if SQLite is compiled using the SQLITE_ENABLE_STMT_SCANSTATUS compile-time option"
(`c3ref/stmt_scanstatus.html`). Amphora asks at prepare rather than at compile, so the same binary
measures and serves. The q-error, max(est/act, act/est), is Moerkotte, Neumann and Steidl's (PVLDB
2(1), 2009).

**Evidence.** `crates/engine/tests/row_counts.rs`: on a(100), b(3 a row of a), c(2 a row of b),
analyzed, the three-table join's prefixes count 100, 300 and 600, each estimate exact and its
q-error 1; a filter keeping a tenth counts 10, 30, 60; a correlated subquery counts its 100 calls
and their 300 rows; a hash join counts its build; a second run counts from nothing; a connection
that didn't ask has no `CountRows` in its program; and every counter's estimate is EXPLAIN QUERY
PLAN FORMAT=JSON's for the same table. The bench's JOB and TPC-H runs (`bench/planner/`) read every
counted estimate back in the EXPLAIN plan of the same query.

**Falsified by.** A counter whose rows differ from the true size of its prefix (a query whose
prefix, run on its own as a COUNT(*), gives another number), or a statement on a connection that
didn't ask that runs slower than before the change (the bench suite's 50 statements, best of 3).

## 13. One estimate per joined set

**Decision.** A join's row estimate is a property of the set of tables it joins
(`optimizer/cardinality.rs`, `JoinedSetRows`): computed from the predicates over that set, once per
`TableMask` and memoized for the search (at most its own subsets: 8,192 sets, past which a set is
computed again, to the same number), and given to every order and every access path the search
tries for the set. An access method changes only what reading the set costs; the rows a seek
returns per outer row stay the cost model's (`estimate_rows_per_seek`: a unique key's one row,
ANALYZE's rows per key prefix, a GLOB's implied range) and no longer reach any output estimate.

Each WHERE term counts once, on its home table: its most selective constraint's, ties to the first
table by name, not by place in FROM, and never by the other terms, so adding a term moves no other.
A term with its table on both sides, which gives a constraint for each side, counts once there.
Selectivities combine through the same `constraints_selectivity` one table's filters use, with one
change: only bounds by a constant (a literal, a parameter, an uncorrelated subquery) make a closed
range together; a bound by a column, of the same row or another table (`x.c <= y.d`), is a join
predicate and stands alone, so it counts the same on whichever table it is counted.

Outer joins by their semantics, each by the Poisson model the planner already took for a match
count: a LEFT JOIN keeps each left row's matches or the row once (`m + e^-m`, `m` the right table's
rows its ON terms keep); a RIGHT JOIN keeps the matches and the right rows no left row matches; a
FULL JOIN keeps both sides' unmatched rows. A WHERE filter on the right table counts what it
restricts beyond the ON terms (`sel(ON and filter) / sel(ON)`), so the matches are never more than
the inner join's rows: LEFT and RIGHT are never below the inner join and FULL never below either. A
RIGHT or FULL JOIN is a reordering barrier both ways, so the rows before it are a set of their own,
combined with it. Semi and anti joins keep or drop left rows and never add any: their tables and
the terms that read them (their unnested EXISTS conditions) multiply by 1. A derived table grouped by
exactly the columns `=` terms equate to other tables' values (a scalar subquery unnested,
`unnest.rs`, by a LEFT JOIN or, once a null-rejecting term has made it one, an inner join) gives one
row for each binding of them, as the correlated form it replaced does.

A term no row passes leaves none: a NULL, one that settles to false as SQLite settles an AND or OR
of literals (`simplified_and_or`, `literal_truth`), a comparison with a NULL literal by `=`, `<>`,
`<`, `<=`, `>` or `>=`, or an AND with such a side. In WHERE it leaves no rows in any set; in a
join's ON it makes that join's right table match nothing. So does `c IS NULL`, however written
(`NULL IS c`, `c IS NOT DISTINCT FROM NULL`), of a column no row of its table holds NULL in (the
rowid, a NOT NULL column), which is true only of a row a join null-extended: on a LEFT JOIN's right
table it keeps the left rows that matched nothing; on a table no join null-extends (not a LEFT or
FULL JOIN's right table, nor before a RIGHT or FULL JOIN) it keeps none; as an ON term of the
table's own outer join it matches none. A plan made impossible by a constant-false WHERE estimates
each table access at 0 rows.

EXPLAIN QUERY PLAN FORMAT=JSON's `output_rows` is the rows the top-level statement is estimated to
return (`ProgramBuilder::output_rows`), never an uncorrelated subquery's whose tables the program
holds too: a SELECT's join after grouping and aggregation, cut to a literal LIMIT
(`estimated_output_rows`); VALUES its rows; a SELECT of no table one row, none under a WHERE never
true; a compound SELECT its arms', UNION ALL and UNION adding, INTERSECT taking the smaller, EXCEPT
its left arm's, cut to a literal LIMIT (`optimizer::select_output_rows`, `compound_output_rows`, the
same fold a FROM-clause subquery's rows take). Each grows or stays as its join's rows grow, so a
more restrictive query is never estimated at more rows for its shape alone. An ON term's BETWEEN is
rewritten to its two comparisons as a WHERE term's is (`planner.rs`, `split_rewritten_ands`). Loops
run over a set's tables and its RIGHT or FULL JOINs, bounded by the mask; nothing recurses.

**Why.** The estimate was the chosen access path's: rows per outer row from the seek or scan
picked, times the filters it didn't consume, with each term's share moved around by the order its
tables came in (`rows_after_join`, `completed_terms_selectivity`). So the same join got other
estimates in other orders and with other access paths, and the search compared plans of one set by
numbers that differed only in how they were reached. The oracles' CERT check found the symptoms
(cause AG: a FULL JOIN estimated below its inner join; an index seek's estimate against two
filters'); the order check this decision adds found them at their cause.

**Sources.** PostgreSQL computes a join relation's rows once and keeps them for all its paths
(`set_joinrel_size_estimates`, `src/backend/optimizer/path/costsize.c` lines 5671 to 5680 at
126976a8: "In theory we should get the same answers no matter which pair is provided; ... and
anyway we must keep the rowcount estimate the same for all paths for the joinrel"). CockroachDB's
memo: "Because memo groups contain logically equivalent expressions, all the memo expressions in a
group share the same logical properties" (`pkg/sql/opt/memo/memo.go` line 62), the row count among
them (`statistics_builder.go`). DuckDB caches the cardinality of each relation set
(`src/optimizer/join_order/cardinality_estimator.cpp` at 1641475, line 188:
`reference_map_t<JoinRelationSet, CardinalityHelper> relation_set_2_cardinality;`,
`EstimateCardinalityWithSet` at line 907 reading it before computing), after Ebergen's thesis: "the
cardinality is also represented as a set of relations" (docs/research/planner.md, findings f15, f16
and f39, kiln grade A). Only a bound by a pseudo-constant joins a range: PostgreSQL's
`clauselist_selectivity` (`src/backend/optimizer/path/clausesel.c` lines 205 to 231 at 126976a8)
looks for "a restriction clause with a pseudoconstant on one side" of one relation before it pairs
`<` and `>` clauses into a range (`addRangeClause`), and merges any other clause's selectivity
"generically" (f16, f17, A). The outer-join bounds are the operators' definitions, SQLite's in its
requirement R-24610-05866 (`test/e_select.test` line 622): for a LEFT JOIN, "after the ON or
USING filtering clauses have been applied, an extra row is added to the output for each row in
the original left-hand input dataset that does not match any row in the right-hand dataset"; a
RIGHT JOIN does the same for the right side's rows and a FULL JOIN for both. The order check
follows CERT's idea (Ba and Rigger, ICSE 2024, f21): a property of the estimator two forms of one
query must share, checked without knowing true sizes. An IS NULL of a never-NULL column is SQLite's
"NOT NULL strength reduction optimization" (`resolveExprStep`, release 3.54.0's `sqlite3.c` as
`bench/oracles/sqlite.sh` downloads it, lines 111487 to 111490: `"expr IS NULL" --> "FALSE"` "if we can
prove that "expr" is never NULL", in a WHERE clause), whose proof `sqlite3ExprCanBeNull` (line 115849) takes
from a column's `notNull` and the `EP_CanBeNull` mark a column of an outer join's right side carries;
its WHERE loop builder likewise ignores "IS [NOT] NULL constraints on NOT NULL columns" (line 172939).

**Evidence.** `crates/engine/tests/planner.rs`: `every_order_of_an_inner_join_gets_one_estimate`
(random inner joins of two to six tables, filters and join terms, with statistics and without, each
in up to 24 FROM orders and each order forced by CROSS JOIN), `an_outer_join_is_estimated_at_least_its_inner_join`,
`a_constraint_seeks_or_filters_to_one_estimate` and `an_on_term_and_a_where_term_give_one_estimate`
failed on dbef789 (one join estimated 1e11 in one order and 1e6 as written; a seek 1e9 and the same
join NOT INDEXED 1e11; a LEFT JOIN 2,500 under its inner join's 3,000) and pass, each shape the
oracles found later added where it first failed. The oracles' order check
(`bench/src/oracle/checks.rs`, `Case::Order`), which plans an inner join of two to four tables with
its FROM list in every order and each order forced by CROSS JOIN, failed 33 of its 435 checks on
dbef789 and none of 502,632 and 530,789 in two campaigns of every oracle on d762dc2, seeds 11 and
12, 1,800 s each (`bench/records/2026-10-26-adas-mac-studio-2-order-check/seed11/summary.json` and
`seed12/summary.json`; an earlier pair of the same campaigns, 375,206 and 385,255, none failed, in
`2026-10-26-adas-mac-studio-order-check.md`; the causes in `bench/oracles/findings.md`). CERT, the
check that found cause AG: all 71 findings of dbef789's run (seed 20261025, 300 s) pass on d762dc2,
and d762dc2's own run found 6, each failing on dbef789 too. The campaigns' other 41 findings
replay alike on both builds (34 fail on both, 7 pass on both: `replay.jsonl` beside the summaries):
the change brings none.
The suite's 52 statements plan as before at 200,000 and at 20,000 items (`--suite --explain`, release
builds of dbef789 and d762dc2), and every statement checked compiles to the same bytecode. Release
binaries of both on adas-mac-studio-2, the whole suite, four rounds in ABBA order, each run started
at load under 2 (`bench/records/2026-10-26-adas-mac-studio-2-joined-set-rows.md`): 94 of the 104
statement and client pairs within 3% or faster by best of 4; the ten below run unchanged programs
and lie within their own binary's spread there (`table_count` at 32 clients 1,691 to 28,585 a
second across the before binary's four runs), and the point reads, alternated tightly, measure 0.984
to 1.004 by median. Measured again (the same record), each of the ten is within 3% too: eight in a
run of the ten alone (best of 4), and `insert_batch_100` and `table_count` at 32 clients, whose
ratio moves both ways from run to run (0.895 to 1.074 and 0.640 to 1.110 by best in four runs), in
eight rounds of their own (1.074 and 1.110, medians 1.002 and 1.013); both compile to the same
bytecode on either binary.

Step 5 of the planner's plan (2026-10-27) made the statement's own `output_rows`, the estimate CERT
reads, the rows of every shape, and added the IS NULL rule above. Every CERT finding of the oracles'
campaigns 1 to 4 and the injected run (126) and of step 5's own runs (5) passes `--replay-check` on
10f2461, and six runs of every oracle there (seeds 11, 12 and 13 and three nights' seeds, 1,800 s
each, on adas-mac-studio-2) fail none of 2,607,858 CERT checks, none of them a query planned without
an estimate (`bench/records/2026-10-27-adas-mac-studio-2-cert-clean/`, `third-runs/*/summary.json`
and `replay-cert.tsv`). `crates/engine/tests/oracle_findings.rs` holds a test for each cause CERT
found, each failing before its fix: `a_hash_joins_estimate_is_the_joins` (AF),
`a_full_join_is_estimated_at_least_its_inner_join` and `an_always_true_conjunct_raises_no_estimate`
(AG), `an_is_null_of_a_column_never_null_keeps_no_inner_row` (BN),
`a_multi_index_or_keeps_the_other_terms_selectivity` (P1), `a_term_counts_where_the_join_completes_it`
(P2) and `bounds_read_apart_make_the_same_range` (P3).

**Falsified by.** Two orders or two access paths of one inner join whose estimates differ (the order
check, run nightly); an outer join estimated below its inner join, or a semi or anti join above its
left side; a query the new estimates send to a slower plan than the old ones chose, measured (the
estimate of a set no longer favours the path that happens to estimate fewer rows for it, so a choice
that rested on that difference may move).

## 14. A row-value range seeks the index its columns lead

**Decision.** A comparison of two row values by `<`, `<=`, `>` or `>=`, `(a, b, c) > (?1, ?2, ?3)`,
the first element of one side a plain column of the table and the other side reading no subquery,
bounds that column at or past the other side's first element, whatever the operator
(`optimizer/row_value.rs`, `constraint`). The bound is implied by the term (`Constraint::implied`,
decision 3): the comparison still filters every row, so a seek on the bound consumes nothing, and
the estimates count the term as they did before, the bound for nothing. A search of an index whose
range the bound is takes as many of the row's elements as the index's columns from the range's on
match, one by one (`extend_row_bounds`): the element is the index's next column, sorted as the
first, compared by the index column's collation and by the column's own affinity (SQLite's
comparison affinity of the element and the column), and not of a custom type, whose index order is
its encoding's. The search's key becomes that row (`SeekKeyComponent::Row`): the scan starts at the
first key at or past it and, bounded on the other side, stops past it, so a keyset page reads from
the row at the cursor and stops at its LIMIT. A NULL first element leaves no row, as a NULL scalar
bound does; a NULL later one only positions the seek among rows the comparison makes NULL. EXPLAIN
QUERY PLAN words it as SQLite does: `(family,key,rev)>(?,?,?)`.

What it leaves to the cost model and the filter, unlike SQLite: an automatic index and a skip-scan
keep the bound on the first column, and the rowid is not one of an index's columns here (SQLite's
`nColumn` counts it). A page that reads columns the index lacks seeks once ANALYZE has counted the
table (4,000 versions: SQLite's search). Without statistics it seeks too under a literal LIMIT,
whose rows stop the read (decision 16), and under a LIMIT that is a parameter, costed to the
q-middle of its rows (decision 17). The lexicographic OR chain (`a > ?1 OR (a = ?1 AND b > ?2) OR ...`) stays a filter,
as it does in SQLite: a keyset is written as the row value, its first page apart.

**Why.** The unify audit's finding 3 (kiln goal 59833ddf373583f4ba3a6d9e4844f1a8): keyset
pagination over a composite primary key, each page of 8 bounded by the last row's key and ordered by
it, was a scan of the key's index from its first entry, every row before the page read and filtered
again on every page, so a pass over the table took time quadratic in its rows: 0.02, 0.09, 0.34 and
1.34 s at 2,000, 4,000, 8,000 and 16,000 rows on the rowid versions shape through turso_core, and
0.06 to 3.20 s on a WITHOUT ROWID one at unify's 542f5a9 (the goal's measurements). The planner
never took a row value for a constraint: `as_binary_components` leaves row values to the expression
code, so no access path saw one.

**Sources.** SQLite 3.54.0's `sqlite3.c` (as `bench/oracles/sqlite.sh` downloads it):
`exprMightBeIndexed` (line 168736) takes a vector inequality for a term on its first element: "If
this expression is a vector to the left or right of a inequality constraint (>, <, >= or <=),
perform the processing on the first element of the vector"; `whereLoopAddBtreeIndex` gives the range
`whereRangeVectorLen`'s count of elements (line 173074), which takes them while each passes its
"Test if comparison i of pTerm is compatible with column (i+nEq) of the index" (line 172789): the
column itself, "the sort order of the index column is the same as the sort order of the leftmost
index column", `sqlite3CompareAffinity` of the element and the column equal to the column's
affinity, and the index column's collation; `sqlite3WhereCodeOneLoopStart` loads the row
(`codeExprOrVector(pParse, pRight, regBase+nEq, nBtm)`), tests its first register alone for NULL
(`OP_IsNull, regBase+nEq`, lines 166666 and 166769) and keeps a vector's term, seeking at or past it:
`if( sqlite3ExprIsVector(pRight)==0 ){ disableTerm(pLevel, pRangeStart); }else{ startEq = 1; }`
(line 166675; `endEq = 1` at the other end, line 166782). The comparison, sqlite.org/rowvalue.html,
section 2.1: "The overall result of comparison is NULL if it is possible to make the result either
true or false by substituting alternative values in place of the constituent NULLs"; and its use,
section 3.1, "Scrolling Window Queries": "A more efficient approach is to remember the last entry
currently displayed", with `WHERE (lastname,firstname) > (?1,?2)`, "And, assuming there is an
appropriate index, it does so very efficiently — much more efficiently than OFFSET."

**Evidence.** `crates/engine/tests/row_value_seek.rs`, four of whose five tests failed on c87e332
(the fifth, one estimate, held there as well) and all pass with the change:
`a_row_value_bound_seeks_the_key_from_the_cursor`, ten forms of kiln's keyset page on its versions
table (each operator, the row on either side, both directions, a prefix of the key, an equality then
a row, both bounds, the value column through the named index and, after ANALYZE, unnamed), planned
word for word as SQLite 3.54.0 plans them, each seeking the key's index by the whole row with no
Rewind or Last (c87e332: `SCAN versions USING COVERING INDEX sqlite_autoindex_versions_1`);
`a_keyset_pass_reads_each_row_once`, a pass over 4,000 versions in pages of 8, forwards and
backwards, every row once in order within LIMIT + 2 cursor positionings a bounded page (c87e332:
1,005,992 positionings forwards, against 5,000); `a_row_value_range_answers_as_sqlite_does`, 125
bounds (NULL in each place) by each operator in five forms on four indexes, 10,000 reads through the
index, each SQLite's row-value answer (a model that SQLite 3.54.0's shell answers alike for all 500
bounds and operators) and the table's NOT INDEXED one, the row sought on three of the indexes and
never past a column of the other direction on the fourth;
`a_row_value_bound_converts_as_its_comparison_does`, SQLite 3.54.0's counts and rowid sums for eight
bounds of other types or with NULLs by each operator, sought by all three columns as SQLite seeks
them; `a_row_value_bound_seeks_or_filters_to_one_estimate`, one estimate sought or NOT INDEXED, and
in a join in either FROM order, so neither CERT nor the order check can see the bound (no campaign
was run: the cardinality code skips implied constraints). The planner's other tests (`planner.rs`,
`records_plans.rs`, `glob_range.rs`, `skip_scan.rs`, `top_n.rs`, `row_counts.rs`, `in_subquery.rs`,
and `oracle_findings.rs` with each CERT cause's test) pass unchanged. A full pass in the row form at
2,000, 4,000, 8,000 and 16,000 versions (a scratch test of the debug build on adacachy, 2026-10-29,
capped beside production, so the shape is the claim, not the times): on c87e332 252,992, 1,005,992,
4,011,992 and 16,023,992 cursor positionings and 0.13, 0.50, 1.99 and 7.86 s, quadratic; with the
change 2,242, 4,492, 8,992 and 17,992 positionings (nine a page of eight) and 0.013, 0.015, 0.026
and 0.056 s, linear. The OR chain is the same scan on both (16,023,992 positionings at 16,000
versions).

**Falsified by.** A row a row-value seek misses or adds: an element whose type, NULL, collation or
order the matching of elements misjudges (`a_row_value_range_answers_as_sqlite_does` and the
oracles' differential check would show it); a keyset page the seek makes slower than the scan it
replaced; a pass whose cursor positionings grow faster than its rows.

## 15. A WITHOUT ROWID table's primary key is the table: ordered, covering, read through its cursor

**Decision.** A WITHOUT ROWID table's primary key, the index on the table's own root page
(`Index::is_without_rowid_primary_key`), is the table's B-tree: it orders the table, holds every
column, and has no row behind it. So read whole, the table comes in its key's order, and an ORDER BY
of the key, of its reverse or of a prefix of it needs no sort (`btree_access_order_consumed` takes
the key's columns where no index is named). Its cost counts no lookup behind it, and its leaves hold
the table's rows (`IndexInfo { covering: true, rows_per_leaf_page: the table's }`). A SELECT reads
it through the table's own cursor, which is keyed by that key (`JoinedTable::
reads_primary_key_through_table`): one descent finds the row and every column of it, where an index
cursor beside the table's found the row and a `Found` on the table's cursor found it again. Every
column is read from the table's cursor at the table's own position, so a key whose columns neither
lead the table nor keep its order reads right. EXPLAIN QUERY PLAN words a scan of it as SQLite does,
`SCAN t`, and a search `SEARCH t USING PRIMARY KEY (...)`.

What it leaves: a write planned through the key (DELETE, UPDATE) keeps its index cursor beside the
table's, and its `Found`; so do the unmatched-row pass of a RIGHT or FULL JOIN and a multi-index OR's
branches, which open their own cursors. Each is right, finding the row twice.

**Why.** Verifying the unify audit's finding 3 natively (kiln goal 3ba4c454cf268ce1a8993a51dfc46a54)
on Codex's canonical engine e2e67247: kiln's records import on records_versions (WITHOUT ROWID, as
kiln's p6_records migration makes it) planned its first page and every next page, `WHERE
(family,key,rev) > (?1,?2,?3) ORDER BY family,key,rev LIMIT 8`, as SCAN + USE SORTER FOR ORDER BY,
and `family > ?1` too: a pass over 2,000 versions made 500,000 cursor positionings, 250 a row, in
519 ms. Equality-prefixed ranges sought the key. Three causes: the order check gave a table's own
B-tree rowid order only ("Without an index, only rowid order is available."); the covering check
counted the key's declared columns only, so the key looked to need a lookup per row and lost to a
scan and a sort; and a search through the key read each row a second time.

**Sources.** SQLite 3.54.0's `sqlite3.c`: `convertToWithoutRowidTable`, "(5) Add all table columns to
the PRIMARY KEY Index object so that the PRIMARY KEY is a covering index", `pPk->isCovering = 1;` and
"The root page of the PRIMARY KEY is the table root page" (lines 129128 to 129251);
`whereLoopAddBtree` reads a covering index alone, `pNew->wsFlags = WHERE_IDX_ONLY | WHERE_INDEXED;`
(line 173836). sqlite.org/withoutrowid.html: "A WITHOUT ROWID table is a table that uses a Clustered
Index as the primary key", and querying a column "only involves a single binary search into the main
B-Tree". SQLite 3.54.0 plans kiln's shapes on kiln's DDL as `SCAN records_versions`, `SEARCH
records_versions USING PRIMARY KEY ((family,key,rev)>(?,?,?))` and `SEARCH records_versions USING
PRIMARY KEY (family>?)`.

**Evidence.** `crates/engine/tests/without_rowid_seek.rs`, two of whose tests failed on e2e67247 and
all pass with the change: `a_without_rowid_tables_key_orders_and_seeks_its_pages`, eight plans on
kiln's records_versions word for word as SQLite 3.54.0 plans them (e2e67247: `SCAN records_versions`,
`USE SORTER FOR ORDER BY`); `a_keyset_pass_over_a_without_rowid_table_reads_each_row_once`, 4,000
versions in pages of 8, forwards and backwards, every row once in order within LIMIT + 2 cursor
positionings a page (e2e67247: 2,000,000 positionings forwards, against 5,000);
`a_row_value_range_on_a_without_rowid_key_answers_as_sqlite_does`, keys ascending, descending and
mixed, 125 bounds (NULL in each place) by each operator in both orders, written as the row and as
`(+a, b, c)`, and an equality then a row: 7,500 reads, each SQLite's row-value answer;
`a_without_rowid_key_seeks_or_filters_to_one_estimate_and_answer`, one estimate sought or NOT
INDEXED, and six bounds of other types or with NULL answering alike sought and NOT INDEXED;
`a_without_rowid_key_reads_every_column_where_its_columns_do_not_lead`, every column of `v, c, b, a`
keyed on (a, b, c) through the key as NOT INDEXED reads it. `row_value_seek.rs` and
`implicit_rowid_seek.rs` pass unchanged. kiln's import pass natively (a scratch test of the debug
build on adacachy, 2026-10-30, capped beside production, so the shape is the claim, not the times):
2,242, 4,492, 8,992 and 17,992 cursor positionings at 2,000, 4,000, 8,000 and 16,000 versions, 1.12
a row, and 7.7, 13.9, 27.3 and 56.4 ms, linear, where e2e67247 made 500,000 at 2,000 in 519 ms.

**Falsified by.** A row a scan or search of the key misses, adds or misorders (the differential
test and the oracles' checks would show it); a column read wrong through the table's cursor; a
write planned through the key that loses its row; a key access slower than the scan it replaced.

## 16. An ordered read under a literal LIMIT stops at its rows

**Decision.** A read of one table has a row goal: the rows a literal LIMIT and a literal OFFSET
keep (`JoinPlanningContext::row_goal`, set in `find_select_plan_form`). It has one only with no
grouping, aggregate, window or DISTINCT between the read and the LIMIT. An index read in the
query's order then costs a seek, and the goal's rows over what the remaining WHERE terms keep of
them (`estimate_ordered_cost_to_goal`), rather than its whole range. An unordered candidate still
pays for its whole read and its sort.

The goal holds only when every WHERE term left to evaluate is one of the table's constraints, whose
selectivity the model knows. A term it can't estimate could make the read reach its goal late or
never, the trap a row goal sets in PostgreSQL, so such a read keeps the whole range's cost.
A join gives no goal, since the goal doesn't measure its fan-out. A `LIMIT ?` or `OFFSET ?` gives
the goal of decision 17.

**Why.** The unify audit's E5 (kiln goal c64d544897cf8adca2c153ed05e2c528). On a rowid table keyed
by (family, key, rev), through its primary key's autoindex or an explicit index on those columns,
three reads planned as SCAN + USE SORTER FOR ORDER BY and read all 4,000 rows for 8:
- `ORDER BY family, key, rev LIMIT 8` past a row value;
- the same past a family;
- the same from the start.
SQLite 3.54.0 searches or scans the index for each.

The model saw the index read's whole range and no LIMIT. Without statistics it takes the table at a
million rows whose pages the cache can't hold, and the index lacked the value column, so each row's
lookup cost a page read (decisions 5 and 11). The read cost about 371,709 past a bound and 929,109
from the start, against 37,888 to 62,863 for a scan and a sort. The LIMIT reached only the output
estimate (decision 13), and `find_table_access_plan` passed it to no access method but a custom
index method's.

**Sources.** PostgreSQL's planner (`src/backend/optimizer/plan/planner.c`) costs a query's paths
for the rows a LIMIT retrieves. `grouping_planner`'s `tuple_fraction` is "the absolute number of
tuples expected to be retrieved (ie, a LIMIT specification)" when it is 1 or more. It is set from
`preprocess_limit`, and the final path is `get_cheapest_fractional_path(final_rel,
tuple_fraction)`.

SQLite's `whereSortingCost` (`src/where.c`) takes the LIMIT as its M where it is smaller ("Use the
LIMIT for M if it is smaller"). SQLite reaches the index through its weights, where a lookup costs
about as much as a row of a scan; Amphora's model prices a lookup as a page read, so it needs the
goal to see the read stop. SQLite 3.54.0 plans the six reads on both indexes as `SEARCH rp USING
INDEX sqlite_autoindex_rp_1 ((family,key,rev)>(?,?,?))`, `SCAN rp USING INDEX
sqlite_autoindex_rp_1` and `SEARCH rp USING INDEX sqlite_autoindex_rp_1 (family>?)`, and alike on
`ri_k`.

**Evidence.** `crates/engine/tests/rowid_index_order.rs`,
`an_ordered_read_under_a_limit_uses_a_rowid_tables_index`. It failed at 327a8f9 with
`SCAN rp` and `USE SORTER FOR ORDER BY`, where SQLite searches `sqlite_autoindex_rp_1`. It now
passes: the six plans are SQLite 3.54.0's word for word, at most 16 rows are read for the 8 (4,000
before), and the answers are those of the same reads NOT INDEXED. `row_value_seek.rs`, `planner.rs`,
`top_n.rs`, `implicit_rowid_seek.rs`, `without_rowid_seek.rs`, `without_rowid_dml.rs` and the
engine's library tests pass unchanged.

**Falsified by.** An ordered index read under a literal LIMIT that does more work in measurement
than the scan and sort it beat. That happens if a constraint's selectivity is far from its rows,
for instance one correlated with the index's order, so that the read passes far more rows than its
goal before it fills the LIMIT.

## 17. An ordered read under a LIMIT known only as it runs stops at the q-middle of its rows

**Decision.** A read of one table that decision 16 would give a row goal, but whose LIMIT or OFFSET
is not a literal (a bound parameter, as kiln writes every LIMIT: `LIMIT ?`), has a row goal too
(`limit_row_goal`, `cost.rs`'s `RowGoal::AtLeast`). How many rows the statement keeps is not known
at prepare, only the fewest:
- one row, plus a literal OFFSET's, under a LIMIT that is a parameter;
- a literal LIMIT's under an OFFSET that is one.
The most is all the read keeps: its range's rows over what its remaining terms keep. An index read
in the query's order is costed to the q-middle of the two, the square root of their product, never
fewer than the fewest. An unordered candidate pays for its whole read and its sort, as under
decision 16, and the goal holds under the same conditions. A negative literal LIMIT is still no
limit, and a negative literal OFFSET still leaves no goal.

**Why.** The unify audit's E5 (kiln goal c64d544897cf8adca2c153ed05e2c528), checked again at main
29be700 (`docs/research/parameter-limit.md`). Every keyset page under a literal LIMIT planned as
SQLite 3.54.0 plans it, through a rowid table's autoindex, an explicit index and a WITHOUT ROWID
key, but kiln writes no literal LIMIT: it binds each one (`crates/kiln/src/records.rs`, `ORDER BY
v.key LIMIT ?6`). Under `LIMIT ?` ten of the probe's reads still planned as SCAN + USE SORTER FOR
ORDER BY where SQLite searches or scans the index:
- past a row value, without statistics;
- past the first column, and from the start, with or without them.
So did kiln's conversation feed on kiln's own DDL, `rev > ?1 ORDER BY rev LIMIT ?2` through
`records_by_rev` (`crates/kiln/src/convo/sql.rs`): each feed page read and sorted every record
line. With no goal, an ordered read was costed for its whole range, a page read for each lookup of
the million fallback rows: about 371,709 past a bound against 37,888 for a scan and a sort.

The other engines' ways don't fit:
- Plans here are made at prepare and kept: "new statistics and a parameter's value never re-plan
  it" (`bench/planner/audit/08-fixed-at-prepare.sql`). So PostgreSQL's custom plans from bound
  values would not be what EXPLAIN QUERY PLAN shows for the statement kiln prepares.
- PostgreSQL's 10% of the result still leaves the read from the start a scan and a sort in this
  model: 100,000 lookups, about 92,000 against 62,863. Past a bound it wins by about 2%.
- SQLite's weights price a lookup near a row of a scan, where decision 5 measured it as a page
  read.
The q-middle takes no constant. Whatever value is bound, the rows the read keeps lie between the
fewest and all its range keeps. The q-middle is off from them by a factor of at most the square
root of their ratio, where 10% of the read is off by a tenth of its rows for a page of one row.

**Sources.** SQLite 3.54.0 (github.com/sqlite/sqlite, tag `version-3.54.0`):
- `src/select.c`, `computeLimitRegisters` (lines 2603-2611) takes a LIMIT into the estimates only
  as an integer literal: `if( sqlite3ExprIsInteger(pLimit->pLeft, &n, pParse, 0) ){` sets
  `nSelectRow` and `SF_FixedLimit`, and `assert( WHERE_USE_LIMIT==SF_FixedLimit );` (line 8436).
- Under `LIMIT ?` it still reads the index in order, because its weights make a full scan 3.0×N:
  "The 3.0 factor is an extra cost designed to discourage the use of full table scans, since index
  lookups have better worst-case performance if our stat guesses are wrong" (`src/where.c`, lines
  4158-4161).

PostgreSQL (`00d33e82bbb087d0f77fb5f49475885db62d1e37`):
- `src/backend/optimizer/plan/planner.c`, `preprocess_limit` (lines 2946-2954): "However, if it's
  not a constant LIMIT then we have to guess; for lack of a better idea, assume 10% of the plan's
  result is wanted."
- `src/backend/utils/cache/plancache.c`, `choose_custom_plan` (line 1211): "Generate custom plans
  until we have done at least 5 (arbitrary)".

Moerkotte, Neumann and Steidl, "Preventing Bad Plans by Bounding the Impact of Cardinality
Estimation Errors", PVLDB 2(1):982-993, 2009, section 2: the q-error, which is "multiplicative in
nature", and the q-middle, the square root of a set's least times its greatest, of which "the
q-middle minimizes the q-error".

**Evidence.** `crates/engine/tests/rowid_index_order.rs`, three tests that failed at 432a2ab and
pass with the change:
- `an_ordered_read_under_a_parameter_limit_uses_a_rowid_tables_index`: decision 16's six reads with
  their LIMIT and bounds bound. They plan as SQLite 3.54.0 does, word for word, where 432a2ab
  planned `SCAN rp` and `USE SORTER FOR ORDER BY`. Each reads at most 16 rows for its 8, and
  answers as the same read NOT INDEXED.
- `a_keyset_pass_with_a_bound_limit_reads_each_row_once`: a pass over 4,000 rows in pages of 8,
  one statement re-bound for every page, forwards and backwards. Every row comes once, in order,
  and each page reads at most LIMIT + 2 rows, where 432a2ab read 4,000 for a page.
- `kilns_feed_pages_search_records_by_rev`: kiln's feed pages search `records_by_rev` as SQLite
  does, where 432a2ab planned `SCAN records` and a sort.

The probe of `docs/research/parameter-limit.md`, run again with the change (studio-3, 2026-11-01):
of its 60 reads SQLite 3.54.0 plans the same way, 54 plan word for word as SQLite does, against 44
at 29be700. The 6 left are the whole-range reads without a LIMIT, which stay as decision 5 measured
them, and none is under `LIMIT ?`. The planner's other tests pass unchanged:
- `planner.rs`, `records_plans.rs`, `row_value_seek.rs`;
- `without_rowid_seek.rs`, `without_rowid_dml.rs`, `implicit_rowid_seek.rs`;
- `top_n.rs`, `glob_range.rs`, `skip_scan.rs`, `row_counts.rs`, `in_subquery.rs`;
- `oracle_findings.rs`, with each CERT cause's test;
- the engine's 2,660 library tests.

The bench suite's 52 statements (`amphora-bench --suite --target amphora --explain`, release builds of
432a2ab and 110ad12 on jupiter, 2026-11-02, the bench given the root's patches as bug c7af55d0 asks):
every plan the same before and after. `offset_deep`, the one whose OFFSET is a parameter (`LIMIT 20
OFFSET ?`), stays a scan of items in rowid order, the order its ORDER BY asks.

**Falsified by.** A statement whose bound LIMIT routinely asks for most of a large range, where the
ordered read, a lookup for each row, does more work than the scan and sort it beat. Decision 5
measured that at about 3.4× for a whole 200,000-row table. Or one whose remaining terms keep far
fewer rows than their estimates, so the read passes far more rows than its goal (decision 16's
falsifier). The cure then is planning with the bound values, as PostgreSQL's custom plans do, not
another constant.

## 18. A join's rows divide once for each column its equivalence classes join

**Decision.** PL.1 (docs/research/planner.md section 9, rank 1). The `=` terms between two inner
tables' plain columns, from WHERE or an inner join's ON, put those columns in equivalence classes
(`optimizer/cardinality.rs`, `JoinedSetRows`). A set's rows are its tables' rows after their own
filters, divided by each class's divisor over the set's columns the set's terms join: the
product of the class's domains, all but one. However many terms state a class, a term it already
holds divides nothing more, and every order and access path of a set still gets one estimate
(decision 13): the classes are a union-find over the set's terms, rebuilt for each set the memo
computes.

A column's domain is a count where one is kept: a unique key's rows (the rowid, a primary key of
one column, a unique index of one column and no WHERE), else the distinct values `sqlite_stat1`
gives where an index without a WHERE leads it, at most its table's rows. Otherwise its table's
rows stand in for one, the column taken for a key. Which domain the divisor leaves out:
- A class of counts leaves out its smallest. Under containment the smallest domain's values lie in
  every other's, so each of them meets its share of every table's rows once: the product of the
  rows over the product of the other domains. For two columns that is PostgreSQL's and SQL
  Server's division by the larger distinct count.
- A class with a column of no count leaves out the largest such: a foreign key with no statistics
  joins its table's rows to the key's table about once each, as Ebergen's rule assumes the
  lowest-cardinality column a key.

An `=` of two tables' columns no class takes (an outer join's ON term, a term reading a LEFT
JOIN's right table, any such term of a query with a RIGHT or FULL JOIN, whose barrier decision 13
keeps) counts as before but at its pair's divisor, never `sel_eq_unindexed`'s 0.1. A term reading
a semi or anti join's table multiplies by 1, as decision 13 has it.

A unique key of more than one column (a primary key, a unique index without a WHERE), each of its
columns in a class with another table's column, caps its set: each row of the rest meets at most
one of its rows. So the set has at most the rest's rows times the share of the key's table its
own filters keep. A key of one column needs no cap: its domain is its table's rows, so its class
never divides by less.

No added term raises an estimate, as CERT requires. Each domain is at least one and fixed by its
column, not by the class. A column joining a class multiplies its divisor by at least one: by
itself, or by the domain it replaces as the one left out. Merging two classes multiplies the
product of their divisors by the smaller of their left-out domains. A cap is the rest's estimate,
which no term raises either.

**Why.** Every join term multiplied in its own selectivity (row 8 of the audit, section 7), so the
terms JOB writes to state each class whole divided again and again. 26c equates five `movie_id`
columns by ten terms where four connect them. 17a fell from 456,178 rows to 1.57 at its third
table while its counted rows grew from 41,840 to 148,552. Down the long chains 343 of JOB's 977
prefixes were estimated below one row while counting 1,000 or more. A join on a column no index
leads took 0.1 whatever the tables' sizes.

Ebergen's rule as published takes the class's largest known count, else its smallest table's rows.
That is not monotone. A smaller table's column joining a class of columns without counts lowers
the domain for all of them, and raises the estimate: `a.x = b.y` over two 1,000-row tables with a
10-row `c` beside them gives 10,000 rows, and adding `c.z = a.x` gives 100,000. Leaving out one
domain per class keeps Ebergen's answer for a foreign key to its key and PostgreSQL's for two
counted columns, and stays monotone.

**Sources.**
- Ebergen, "Join Order Optimization with (Almost) No Statistics", MSc thesis, CWI and VU Amsterdam
  (`https://blobs.duckdb.org/papers/tom-ebergen-msc-thesis-join-order-optimization-with-almost-no-statistics.pdf`;
  f39, A): "By assuming all joins are foreign-key primary-key joins"; section 3.1, "the cardinality
  is also represented as a set of relations"; section 3.2, "then the column with the lowest
  cardinality in the equivalence set is assumed to be a primary key". On JOB it took 66% and 95% of
  the intermediate tuples off, and 18% and 25% of the time.
- DuckDB, `src/optimizer/join_order/cardinality_estimator.cpp` at 1641475e (f15, A), above
  `EstimateCardinalityWithSet`: "The estimator starts with base relation cardinalities and divides
  by a denominator assembled from predicate domain groups. INNER equality predicates use transitive
  equality classes, composite same-pair equalities can apply an FK/PK cap". `DomainEstimate::Update`
  keeps the largest of a class's HyperLogLog or exact counts and, for columns without one,
  `fallback_distinct_count = MinValue(distinct_count.distinct_count, fallback_distinct_count);`.
- PostgreSQL, `src/backend/utils/adt/selfuncs.c` at 126976a8, `eqjoinsel_inner`: "Estimate the
  join selectivity as MIN(1/nd1,1/nd2)*(1-nullfrac1)*(1-nullfrac2). This is plausible if we assume
  that the join operator is strict and the non-null values are about equally distributed", then
  `if (nd1 > nd2) selec /= nd1; else selec /= nd2;`.
- SQL Server's containment assumption (f43, A): "Users query for data that exists".
- CERT (Ba and Rigger, ICSE 2024, arXiv 2306.00355; f21, A): "CERT derives a more restrictive
  query (e.g., by replacing a LEFT JOIN with an INNER JOIN) whose estimated number of rows should
  not exceed the estimated number of rows for the original query."

**Evidence.** `crates/engine/tests/planner.rs`:
- `a_join_term_its_class_already_holds_divides_nothing_more`: title and three `movie_id` tables
  joined by JOB's six terms and by the three that connect them. One estimate, within 1.5 of the
  12,000 rows counted.
- `a_chain_of_key_joins_is_estimated_near_its_count`: each prefix of the chain within 1.5.
- `an_outer_joins_column_equality_takes_its_domain`: a LEFT JOIN on an unindexed column, within 1.5
  of its 1,000 rows, where 0.1 gave ten times that.
- `an_added_term_never_raises_a_joins_estimate` (T-PL.1.1): 600 random joins of the order tables,
  with statistics and without, each given one more `=`, range or filter. None rises, and more than
  200 fall.
- Decision 13's tests pass unchanged.

**Falsified by.**
- JOB's 343 prefixes below a row not halving.
- JOB no faster where its estimates come near the counts: its geometric mean over the parent at 1
  or more.
- A CERT or order-check failure the change brings.
- A class whose estimate rises with an added term.

## 19. Joins searched over connected subsets within a counted budget; past it, the fewest rows first

**Decision.** The join search (`optimizer/join.rs`, `compute_best_join_order_with_context`) builds
plans for the subsets `searchable_subsets` finds, a size at a time, in place of every subset of
each size (`generate_join_bitmasks`), and it no longer gives every join of more than 12 tables
(`GREEDY_JOIN_THRESHOLD`) to the greedy search. A subset is a smaller one the search kept, with one
more table that may follow all of it (an outer, semi or anti join's right table after the tables
before it; nothing across a RIGHT or FULL JOIN's barrier), holding every table each of its tables
must follow, and joined to it by a term unless no table that may follow it is: so the connected
subsets, and cross products only where no join term is left, as before. The walk is a loop over a
worklist, each level the sets a table larger than the last, and it counts the subsets of two
tables or more it finds against `JOIN_SEARCH_BUDGET`, 10,000. Past the budget it stops, having done
at most the budget times the table count of checks, and the greedy search orders the join instead.

The greedy search adds at each step, of the tables that may come next, the one whose joined set
has the fewest estimated rows (decision 13's `JoinedSetRows`), the cheaper plan where they tie,
where it took the cheapest plan so far, which counts a table's filter for nothing at the step it
is added. Its three panicking calls are gone: a set of tables none of which may start the join is
a typed planning error, and the plan it grows and the order it extends are held as values, not
unwrapped from options. The joined-set memo keeps 16,384 sets, past the budget's 10,000 and its
single tables, or the greedy search's n² sets.

**Why.** Planner audit rows 2, 3 and 16 (`docs/research/planner.md`, rank 3). Of JOB's 113
queries, Amphora was the faster on the 28 of 10 to 12 tables, which the search planned (SQLite's
time over Amphora's 1.813 at step 3), and the slower on the 9 above 12, which the greedy search
planned (0.405): 33c took 692 ms where SQLite took 30.9. The audit's star of a fact table and 12
dimensions, the last filtered, read the filtered dimension last under the greedy search, at 5.7
times the cost of the order the search of 12 tables finds (`01-left-deep-greedy.out`). And the
walk of every subset grew with 2^n where the subsets a join can use without a cross product grow
far slower: a chain of 12 tables has 78 connected subsets of 4,095, and planning a star of 8 to 12
tables grew 48 times where a chain's grew 7 (`02-subsets-by-size.out`). JOB's 28a to 28c have
2,032 connected subsets and 33a to 33c 1,517, inside the budget; 29a to 29c have 13,246, past it.

The search stays left-deep, a table a step, so execution doesn't change; bushy plans over
connected pairs with a conflict detector are rank 8. The walk enumerates DPccp's connected
subgraphs, not its complement pairs, which a left-deep search has no use for, and on a worklist
where DPccp and DPhyp are stated recursively (CLAUDE.md §11).

**Sources.** Moerkotte and Neumann, "Analysis of Two Existing and One New Dynamic Programming
Algorithm for the Generation of Optimal Bushy Join Trees without Cross Products", VLDB 2006, and
"Dynamic Programming Strikes Back", SIGMOD 2008: DPsize, which the walk of every subset resembles,
runs far above the lower bound that DPccp, enumerating connected subgraphs only, meets
(`docs/research/planner.md` section 2, f22). Neumann and Radke, "Adaptive Optimization of Very
Large Join Queries", SIGMOD 2018: "count the number of connected subgraphs up to the budget of
10,000" and solve exactly within it, rather than switching to greedy at a table count (f23).
DuckDB's enumerator leaves its dynamic program for a greedy one past 10,000 pairs
(plan_enumerator.cpp, f24). Leis et al., "How Good Are Query Optimizers, Really?", PVLDB 9(3),
2015: on JOB exhaustive search beat the heuristics even with estimation errors, GOO's plans up to
21 times the optimum (f27); Greedy Operator Ordering joins the pair of least result size (f28).

**Evidence.** Tests that failed at 8d9e353, the commit before the change (fedora, 2026-11-06), and
pass with it (6395771, on adas-mac-studio.local, the engine at 0bfaaee with the change: clippy
`--all-targets -D warnings` clean, `join_search.rs`, `planner.rs` and `records_plans.rs`, and the
engine's 2,665 library tests):
- `thirteen_tables_within_the_budget_are_searched_exactly` (`crates/engine/tests/join_search.rs`):
  the audit's star of a fact table and 12 dimensions, the last filtered, reads the filtered
  dimension among its first two tables, where 8d9e353 read it last.
- `a_join_past_the_budget_is_ordered_by_the_fewest_rows`: a star of 40 tables, 2^39 connected
  subsets, is planned whole by the greedy search with the filtered dimension among its first two,
  where 8d9e353 read it last.
- `no_starting_table_is_a_typed_error` (`join.rs`): a join none of whose tables may start is a
  planning error, where 8d9e353 panicked (`best.expect("no valid starting table")`).

And tests of the walk itself, in `join.rs`:
- `the_connected_walk_finds_the_subsets_the_walk_of_every_subset_keeps`: over 400 random join
  graphs of 2 to 9 tables, with outer, semi and anti joins and RIGHT or FULL barriers, the
  connected walk finds, size by size, exactly the subsets the walk of every subset kept. Below the
  budget the search therefore builds the plans it built before. More than 50 of the graphs have
  ordering constraints and more than 5 are joined only by cross products.
- `subsets_past_the_budget_are_refused`: a chain of 4 tables (6 subsets of two or more) is
  refused under a budget of 5 and walked under 6.
- `a_long_chain_is_searched_and_a_wide_star_refused`: a chain of 63 tables, the most a query
  joins, is 1,953 subsets, inside the budget; a star of 40 is refused.

`scripts/engine-ratchet.toml` falls with it (f6fddfd): `unwrap_used` 452 to 451 and `expect_used`
873 to 871, the greedy search's three calls; `arithmetic_side_effects` 3406 to 3401 and
`cast_possible_truncation` 560 to 559, the walk of every subset.

**Falsified by.** JOB's 9 queries above 12 tables no faster than on the parent; a query past the
budget slower than its parent's plan; a query of 10 to 12 tables planned differently from the
parent (the model test says none is); or a JOB query planned in more than 92.1 ms, which would
say the budget is too large for this search's cost per subset.

## 20. ANALYZE keeps each column's distinct count and a sample of each table

**Decision.** PL.2 (docs/research/planner.md section 9, rank 2). ANALYZE, beside `sqlite_stat1`,
reads each table once more and keeps, in `__turso_internal_column_stats(tbl, col, kind, data)`
(the engine's reserved prefix, which SQLite and earlier builds never consult):
- a HyperLogLog sketch of each stored column (`column_stats.rs`): 2,048 six-bit registers, 1,536
  bytes, with the column's non-NULL and NULL values counted. A value hashes as SQLite compares it:
  an integer within 2^53 as the real it equals, text and BLOB apart. The hash is a fixed function
  (FNV-1a, then MurmurHash3's finalizer), the same on every platform.
- a reservoir sample of each table's rows (Algorithm R): at most 1,024 rows and 1 MiB of their
  records, chosen by splitmix64 seeded with the table's name, so two passes of one build keep the
  same sample. A table whose sample would pass 1 MiB keeps none, its row's data NULL.

The schema loads them with `sqlite_stat1` (`stats.rs`), each table's sample with its common
values counted: those the sample holds at least ten times, the most common first. At planning, a
column keeps as common only those that stand apart from the rest, as PostgreSQL's
`analyze_mcv_list` decides (`common_kept`). The least common value is dropped while its sample
count is within two standard errors and a half of what the values left would give it. So a
column whose values are about equally common keeps none: its estimates are its distinct count's,
the same in every table that holds those values, and no sample's noise moves them. The samples a schema loads stay within
64 MiB of records; a table past it loads none and its estimates take its distinct counts, logged
on `amphora.planner.stats` (`event = "sample_over_budget"`).

An `=` or IN list of a column against constants (`constraints.rs`, `apply_column_stats`) then
takes:
- a common value: its share of the sample;
- another value: the share of the column's non-NULL rows the common values leave, over the
  distinct values they leave, at most the least common value's share (PostgreSQL's
  `var_eq_const`);
- a bound parameter, or a column whose collation is not BINARY: its non-NULL share over its
  distinct count;
- an IN list: the sum of its values' shares, at most one.

A rowid, a one-column key and a unique column keep one row a value, and a join term keeps its
equivalence class (decision 18). Where no index leads a join column, the sketch's distinct count
is its domain (`cardinality.rs`, `column_domain`). When ANALYZE's statistics are loaded, the
connection's prepared statements plan again at their next run (`statement.rs`), as SQLite's
ANALYZE ends with `OP_Expire`. Other connections take the statistics when they next load the
schema, as in SQLite.

**Why.** At step 3, 16 of JOB's 113 first tables were off by 13,417 or more, 15 of them an
equality on `keyword.keyword`, which no index covers: a tenth of the table estimated against one
row counted. TPC-H SF 10's q16 took its IN list of eight sizes for eight rows (1.44 estimated,
296,824 counted), and q12's two shipping modes for two. A statement prepared before ANALYZE kept
the plan it was given without statistics (audit row `08-fixed-at-prepare`).

**Sources.**
- Flajolet, Fusy, Gandouet and Meunier, "HyperLogLog: the analysis of a near-optimal cardinality
  estimation algorithm", AofA 2007: standard error 1.04/sqrt(m), 2.3% at m = 2,048; linear
  counting below 5m/2 with a register empty. A 64-bit hash needs no large-range correction.
- Freitag and Neumann, "Every Row Counts", CIDR 2019, and Umbra (f14, A): updatable HyperLogLog on
  every column, about 2% in 1.5 KB, with a sample of every relation. DuckDB (f15, A) and
  ClickHouse (f42, A) keep HyperLogLog distinct counts per column.
- Vitter, "Random Sampling with a Reservoir", ACM TOMS 11(1), 1985: Algorithm R. Steele, Lea and
  Flood, OOPSLA 2014: splitmix64. Lemire, ACM TOMACS 29(1), 2019: a bounded draw by multiply and
  shift.
- PostgreSQL, `src/backend/utils/adt/selfuncs.c` at 6a3da13d376f, `var_eq_const`: "We approximate
  that all the not-common values share this remaining fraction equally, so we divide by the number
  of other distinct values", then "selectivity shouldn't be estimated as more than the least
  common \"most common value\"".
- PostgreSQL, `src/backend/commands/analyze.c` at 6a3da13d376f, `analyze_mcv_list`: "Exclude the
  least common values from the MCV list, if they are not significantly more common than the
  estimated selectivity they would have if they weren't in the list", the test
  `mcv_counts[num_mcv - 1] > selec * samplerows + 2 * stddev + 0.5`.
- SQLite, `src/analyze.c` at 2a4fb6233696, `sqlite3Analyze`: `sqlite3VdbeAddOp0(v, OP_Expire);`.

**Evidence.** `crates/engine/tests/column_stats.rs`, red on 8289870 and green with the change:
- `an_equality_no_index_leads_is_estimated_from_its_distinct_count` (AC-PL.2.1): JOB's keyword
  table at 13,417 rows, an equality on its keyword within 1.1 of the one row counted.
- `a_frequent_value_takes_its_share_of_the_sample` (AC-PL.2.1): a code holding 8,000 of 20,000
  rows and a rare one holding 60, each within 1.25.
- `an_in_list_is_estimated_as_its_values_shares` (AC-PL.2.2): q16's eight sizes and q12's two
  modes, each within 1.25.
- `a_statement_prepared_before_analyze_plans_again_after_it` (AC-PL.2.3): prepared once again,
  with the estimate a statement prepared afterwards gives.
- `a_table_past_the_samples_bound_keeps_its_distinct_counts` (T-PL.2.1): a 6 MiB table keeps no
  sample, and its equality is still within 1.25 from its sketch.

`without_rowid_analyze.rs`'s `a_without_rowid_keys_statistics_reach_the_planner` (decision 15)
now reads the key's `sqlite_stat1` row through a join's domain. That is where it still decides
the estimate: an equality on the key's first column now reads each table's own sample, which a
WITHOUT ROWID table and its rowid copy draw apart (570.5 against 555.5 rows on the same values).

The unit tests in `column_stats.rs` (T-PL.2.1) check the sketch within three standard errors
from 1 to 1,000,000 values, its encoding back to itself, and the reservoir: 1,024 rows, the same
for the same name, uniform within 40 rows a tenth, and none past its byte bound.

**Falsified by.**
- A keyword prefix of JOB off by more than 1.1 after ANALYZE.
- q16's part off by more than 10, or q12's lineitem by more than 100.
- A plan the new estimates move running slower than its parent's beyond the passes' spread.
- ANALYZE of JOB's or TPC-H SF 10's database taking more than twice its parent's time, measured
  on the bench.

## 21. A table's filters are estimated together on its sample, and otherwise combine by backoff

**Decision.** PL.4 (docs/research/planner.md section 9, rank 4). When the planner sizes a joined
set (`cardinality.rs`, `JoinedSetRows`), each inner table with a sample (decision 20) runs its own
WHERE terms over the sample, once a prepare (`sampled_filters`). The share of the sample's rows
that pass them all is their selectivity together: ranges, LIKE and GLOB, ORs of conjunctions,
IS NULL and correlated columns at once. A term takes part when it reads only this table's
sampled columns and constants: a WHERE term or an inner join's ON term, which the order check
holds to one estimate, including an OR that no constraint homes. The ON terms of the table's own
outer join run apart, and again together with its filters. Outer joins size their matches from
`sel(ON and filters) / sel(ON)`, so an outer join reads the sample for the same terms as the inner
join it restricts to (CERT). The evaluator (`optimizer/sample_filter.rs`) runs comparisons,
BETWEEN, IN lists, LIKE and GLOB, IS NULL, AND, OR and NOT, with SQLite's comparison affinity
and the BINARY, NOCASE and RTRIM collations. An `=` or IN list of constants on a column with a
sketch keeps its column statistics' estimate (decision 20), which tells a common value from a
rare one; the sample would only count it. So does any other term: a parameter, a function, a
subquery, another table's column, or a term that raises an error on a sample row.

Each term is compiled once into a flat program, an operation for each node it holds (a loop over
an explicit stack, CLAUDE.md §11). Each runs once on each sample row (§2), so a table's terms take
their length times the sample's rows. The sample's rows are no more than the table's, so that is
never more than a scan of the table takes to run the same terms. Until 2026-11-11 a term was held
to 64 operations and a table's terms to 1,048,576 steps (64 × 1,024 × 16). Both counts were picked
(Ada, 2026-11-11: "We don't tolerate arbitrary constants, learned and stat derived values only") and
are gone; the parser's bound on a tree's height (`MAX_EXPR_DEPTH`) bounds the one walk down
parentheses and COLLATEs. `a_long_term_is_estimated_on_the_sample`: an OR of 25 one-value BETWEENs,
124 operations, was estimated at 20,000 rows against 10,000 counted (red at 490ea83, local-2); run on
the sample it is within 1.25.

Where at least ten sample rows pass the terms together, their share of the sample is the
selectivity. Where fewer do, it is the share of the rows that pass, at least half a row's, but no
more than the terms' own estimates combined by backoff. So a conjunction no sample row passes
still takes its terms' statistics, and a rare value keeps its distinct count's estimate.

A table's filters then combine by exponential backoff (`cost.rs`, `constraints_selectivity`), in
place of their product: the sampled share, each range (a column bounded from both sides counts
once, as `sel_closed_range`), each LIKE and each other filter. The most selective counts whole,
the next at its square root, then its fourth root and its eighth; the rest are taken as correlated
with these. Equalities, IN lists and terms that read another table still multiply: their
selectivities come from `sqlite_stat1`'s prefixes and the sketches' distinct counts, by which a
whole key's equalities give its one row.

No added term raises an estimate, as CERT requires. A term's treatment depends on its own form,
not on the others. A term added to the sampled ones passes no row the others don't, so their share
falls; across the ten-row line it falls below ten rows' share. Backoff never rises with an added
selectivity: placed k-th, it multiplies by s^(e_k), and each selectivity it moves down a place,
none more selective than it, by at most s raised to the exponent that one loses; those losses sum
to at most e_k (`backoff`'s unit test). The other estimates multiply.

**Why.** At step 3, TPC-H SF 10's q19 part, an OR of three conjunctions, was estimated at 1.2
rows against 239,849, and q06's lineitem, three ranges, was off by 1.9e+03. With its keyword
prefixes exact, JOB's first tables kept a q-error p95 of 66.3 from their LIKE and range filters.
Each range took a fixed 0.4, a closed range 0.005 and a LIKE 0.2, all multiplied as if
independent; an OR no constraint takes was not counted at all.

**Sources.**
- Microsoft Learn, Cardinality Estimation (SQL Server), on CE 120 and higher: "Partially
  correlated (default for CE120 and higher), where cardinality is calculated using a variation on
  exponential backoff, ordering the selectivities from most to the least selective predicate"
  (f17, A). The exponents 1, 1/2, 1/4 and 1/8 over the four most selective are those
  docs/research/planner.md rank 4 gives for CE 120. They were not checked against Microsoft's own
  text here, and the bench is what tests them.
- Oracle's dynamic statistics (f13, D: graded for Han et al.'s counterclaim on sampling across
  joins, which this sample of one table is not): "The database uses recursive SQL to scan a small
  random sample of table blocks", at parse time, for the same conjunctions.
- CERT (Ba and Rigger, ICSE 2024; f21, A): a more restrictive query's estimate should not exceed
  the original's.

**Evidence.** `crates/engine/tests/sample_filters.rs`, red at 885bda9 and green with the change:
- `q06s_ranges_are_estimated_together_on_the_sample` (AC-PL.4.2): 0.2 estimated against 339 counted
  on the parent; within 2 with the change.
- `a_like_is_estimated_on_the_sample` (AC-PL.4.2): 1,600 against 2,494 on the parent; within 1.5.
- `correlated_columns_are_estimated_together` (audit row 5): 1,600 against 5,000 on the parent;
  within 1.25.
- `q19s_or_of_conjunctions_is_estimated_on_the_sample` (AC-PL.4.1): 4,627.7 against 611 on the
  parent; within 2.
- `a_filter_made_more_restrictive_never_raises_its_estimate` (T-PL.4.1): 400 random conjunctions,
  with the sample and without statistics, each given one more term. None rises, and more than 200
  fall.
- `backoff_never_rises_with_an_added_selectivity` (unit, T-PL.4.1).
- `an_inner_joins_on_term_and_a_where_term_give_one_estimate` and
  `an_outer_join_with_sampled_terms_is_estimated_at_least_its_inner_join` hold the oracles'
  findings on seed 11. With only WHERE terms sampled there were 30 order-check findings (an
  inner join's ON term against the same WHERE term). With ON terms sampled but an outer join's
  apart from its filters there were 21 CERT findings, then one. With the joint sample, seeds 11
  and 12 at 300 s each found none in 855,783 checks.
- Decision 20's tests and `records_plans`' whole-key lookup pass, the latter because equalities
  multiply.

**Falsified by.**
- q19's part off by more than 10 at SF 10, or q19 no faster once its part's estimate is near its
  count.
- q06's lineitem off by more than 2.
- JOB's first tables' q-error p95 not under 20.
- JOB's median planning time over twice step 3's 1.9 ms (the sample's run, a table's terms'
  length times its sample's rows, is the cost to watch).
- A statement whose terms' run on the sample takes longer than the statement's own execution
  (the bound above assumes the scan runs the same operations the sample does; an IN list the
  executor probes through an index and the sample walks value by value breaks it).
- A CERT failure in the oracles.

## Decision (lane pl6): No table read whole beneath loops that can give more than one row

**Decision.** PL.6 (docs/research/planner.md section 9, rank 6). A read of a B-tree table by no
key (a scan of the table, or of an index with no constraint seeking it) is planned beneath the
loops before it only when they give at most one row: each reads its table by every column of a
unique key with `=` (the rowid, a primary key or a unique index: `order.rs`'s one-row loop), or
there are none. A hash join's build-only table runs no loop of its own and is left out. Otherwise
the planner reads the table by a key at any cost: a seek of an index, an IN seek, a multi-index
read or an automatic index (`access_method.rs`, `WholeReads::Refused`), or a hash join
(`join.rs`, `join_lhs_and_rhs`), the cheaper of what exists. A seek taken only because the whole
read was refused doesn't keep a seek's preference over a hash join, so a hash join of the same
rows as the scan still wins where the seek reads more. The whole read stays only where none of
these can read the table: no equality joins it to the loops before it (a cross join, a join by a
range), or INDEXED BY and NOT INDEXED leave nothing else. A hash join's probe is a whole read
too: where the loops before its build table can give more than one row, its build input is
materialized from them (the existing key and payload materialization), so the probe is read
once. The read that finds a RIGHT or FULL JOIN's unmatched rows runs once and keeps its choice.

**Why.** An estimate of rows decides how often a whole read runs, and an estimate can be wrong
by any factor: at step 3, 30 of JOB's 113 plans read a table whole beneath a first table
estimated at 0.4 or 1.2 rows by a fixed selectivity (`bench/planner/loops.py` over
`bench/records/2026-10-26-jupiter-planner-job-a.jsonl`). Each first table counted one row, so the
whole read ran once; the audit's `10-no-adaptive`, statistics taken at 10 rows and the tables grown
to 3,000, read q whole for each of p's 3,000 rows. Only a one-row loop bounds the reads by
construction rather than by estimate. A hash join or an automatic index reads the table once
whatever the loops before it give; a seek reads only the rows it finds.

**Sources.**
- Leis et al., "How Good Are Query Optimizers, Really?", PVLDB 9(3), 2015, section 4.1 (f32, A):
  nested-loop joins without an index are "extremely risky" under misestimates, and with them
  disabled "we observed no more timeouts" on JOB under PostgreSQL's estimates
  (docs/research/planner.md section 5, lines 894 and 899 of the paper's text).
- Leis et al. 2015 (f11, A): JOB's estimates are off by orders of magnitude at several joins,
  the misestimate the rule is robust to.

**Evidence.** `crates/engine/tests/nested_loops.rs`, red at c1ce17a (the batch's planner) and
green with the change: `t_pl_6_1_ten_no_adaptive_reads_q_once` (T-PL.6.1), an outer join and the
reverse order, JOB 1a's dimension and facts, a one-row loop that keeps its scan, a cross join and
a range join that keep theirs (the counter finds it, so it is not vacuous), and a chain of hash
joins whose third table is read once. Each test counts as `bench/planner/loops.py` does, whose
own test holds step 3's counts: 30 of JOB's 113 plans, none of TPC-H's 22 at SF 1 or SF 10. A hash
join's build table is listed in its block after the probe (`HASH JOIN q / SCAN p`) but read once,
before the first probe (`Insn::Once`); the JSON plan's `hash_join` step names it (`build`), and the
counter neither counts that read nor treats it as a loop. The JOB and TPC-H measurement against the
parent is the batch's bench.

**Falsified by.**
- A JOB plan with such a read in the change's records (`loops.py` above 0), or a TPC-H plan.
- A plan the rule changes that runs slower than its parent's on JOB, TPC-H or the bench suite
  beyond the passes' spread (research rank 6's falsifier).
- An answer that changes (`answers.py`), or an oracle finding where the forced materialization
  of a hash join's build input is planned.
## Decision (lane pl8): a RIGHT JOIN led by one table is planned as its mirrored LEFT JOIN

**Decision.** PL.8's first rule (docs/research/planner.md section 9, rank 8; audit row 4). Before
the join search, `t0 RIGHT JOIN t1 ON p`, where t0 is the FROM list's first table alone and both
are B-tree tables, becomes `t1 LEFT JOIN t0 ON p` (`optimizer/mod.rs`,
`commute_leading_right_join`): the two tables swap places, t0 takes the join's information as a
LEFT JOIN, and the ON terms' origin names t0. Columns, `*` and USING are bound by table ID before
the swap, and what reads a table's place in the FROM list (`outer_join_may_null_extend`,
`is_left_of_right_or_full_join`) reads it live, so each table is null-extended where it was.

**Why.** A RIGHT JOIN's unmatched rows of its right table are read after every loop, so
`required_lhs_by_table` holds every later table behind it. The audit's
`03-right-full-barrier.out`: `(big RIGHT JOIN mid ON mid.k = big.k) JOIN tiny ON tiny.id = mid.id
WHERE tiny.id = 5` read big and mid ahead of tiny, total_cost 328,300,039,000.3, where the same
query written `mid JOIN tiny LEFT JOIN big` read tiny first at 23,007.0. As a LEFT JOIN only t0
must follow t1, and tiny, whose term reads only mid, moves first.

**Sources.** Moerkotte, Fender and Eich, "On the correct and complete enumeration of the core
search space", SIGMOD 2013 (f25): a right outer join is the left outer join of its operands
swapped, and CD-C reorders outer joins by such rules, correct and complete. Its left-asscom rule,
`(e1 LEFT JOIN e2) JOIN e3 = (e1 JOIN e3) LEFT JOIN e2` when e3's term doesn't read e2, is what
lets tiny move ahead of the LEFT JOIN, which the search already allowed.

The estimate follows (decision 13): an outer join's ON term that reads none of its right table's
columns, only tables before it (`y.c = 3` in `y LEFT JOIN x ON y.c = 3`, which `x RIGHT JOIN y ON
y.c = 3` becomes), was counted nowhere: `constraints_from_where_clause` dropped an outer join's ON
term on every table but its right table, and the right table has no constraint of it. Such terms
now keep their constraints on the earlier tables apart, in `TableConstraints::on_elsewhere`, which
no access path reads. The estimate finds a join's ON terms by their origin among the WHERE terms
and counts those its right table holds no constraint of by those constraints and the earlier
table's sample, at the join (`cardinality.rs`, `on_selectivity`), as a right table's ON terms are
counted. So the RIGHT JOIN turned LEFT keeps its RIGHT JOIN's estimate, and a LEFT JOIN written
that way is no longer estimated as if its ON term were absent.

**Evidence.** `crates/engine/tests/planner.rs`'s `an_outer_join_is_estimated_at_least_its_inner_join`
failed with the swap alone (`t0 RIGHT JOIN t1 ON y.c = 3`: right 10^12, full 10^11) and passes
with the estimate's change. `crates/engine/tests/join_search.rs`: `t_pl_8_1_the_right_join_reads_tiny_first`
(T-PL.8.1) plans tiny first and mid before big; `t_pl_8_2_a_right_join_turned_left_keeps_its_answers`
(T-PL.8.2) answers eight RIGHT JOINs (`*`, a WHERE on the null-extended table, USING, a later inner,
LEFT and RIGHT JOIN, an aggregate, an ON term on the left table) as SQLite does, worked by hand
from the rows.

**Not yet built.** The rest of rank 8: bushy plans over connected complement pairs (DPccp, DPhyp
for terms over more than two tables) on rank 3's explicit stack and budget, a join's result built
into a hash join through `MaterializedBuildInput`, and CD-C's conflict rules in place of
`required_lhs_by_table` for outer, semi and anti joins and for RIGHT JOINs whose left side is a
join (those need a bushy plan: `(a JOIN b) RIGHT JOIN c` is `c LEFT JOIN (a JOIN b)`).

**Falsified by.** An answer that changes against SQLite (the oracles' DQP, TLP and NoREC, and
`answers.py`) for a RIGHT JOIN led by one table; or such a plan slower than its parent's on the
bench suite beyond the passes' spread.

## Decision (lane pl8-3): a LEFT JOIN's right table follows the tables its ON terms read

**Decision.** PL.8, CD-C's conflict rules for the left-deep orders the search makes
(`optimizer/join.rs`, `required_lhs_by_table`). A LEFT JOIN's right table must follow the tables
its ON terms read (CD-C's syntactic eligibility set), no longer every table before it in the FROM
list: `a JOIN e ON e.x = a.x LEFT JOIN b ON b.y = a.y` may read a, b, e. When its ON terms read no
table of its left side (`ON 1`, `ON b.x = 5`), CD-C keeps its whole left side before it, as before.
A semi, anti, FULL, CROSS or LATERAL join's table keeps every table before it, and a RIGHT or FULL
JOIN stays a barrier both ways, as before. Beside it, `JoinN` gains the bushy shape
(`inner_joins`, `JoinN::tree`): runs of its tables joined among themselves first, empty for
today's left-deep plans; the left-deep emitter refuses a bushy plan with a typed error.

**Why.** Moving a LEFT JOIN's right table b ahead of a table e of its left side is the
left-asscom rule `(e1 LEFT JOIN b) op e = (e1 op e) LEFT JOIN b`, which CD-C holds for op an inner,
left, semi or anti join whose term doesn't read b. A table whose term reads b follows b by that
term, and a WHERE term on b runs where its tables are read, after b's NULL row, as SQLite runs it.
Semi and anti joins keep their place for the emitter's sake, not the algebra's: their no-match path
jumps into the row's body (`main_loop/close.rs`), so their tables must be the last loops.

**Sources.** Moerkotte, Fender and Eich, "On the correct and complete enumeration of the core
search space", SIGMOD 2013 (f25): the operator properties table (left-asscom holds among inner,
left outer, semi and anti joins) and CD-C's conflict rules.

**Evidence.** `crates/engine/tests/join_search.rs`'s
`t_pl_8_2_outer_semi_and_anti_joins_keep_their_answers` (T-PL.8.2): twelve queries (a LEFT JOIN
reading only the first table after an inner join, two LEFT JOINs off one table, an inner join
reading the null-extended table, ON terms reading no left table, a WHERE term on the NULL row, an
aggregate, EXISTS and NOT EXISTS over them) answer as SQLite does, worked by hand from the rows.
`optimizer/join.rs`'s `t_pl_8_bushy_shape_holds_left_deep_and_bushy_trees`: a left-deep plan reads
as the chain, nested and side-by-side runs as subtrees, runs that don't nest are refused. Every
planner suite (`planner`, `join_search`, `oracle_findings`, `nested_loops`, `records_plans`, the
optimizer's unit tests) passes unchanged.

**Not yet built.** The emitter's hash join whose build side is a join of tables that aren't the
plan's leading ones (extending `MaterializedBuildInput`), DPccp's connected complement pairs on an
explicit stack with a budget derived from the plan-time target, and semi and anti join tables
placed before other loops (the emitter's no-match path first).

**Falsified by.** An answer that changes against SQLite (the oracles' DQP, TLP and NoREC, and
`answers.py`) for a query with a LEFT JOIN; or such a plan slower than its parent's on the bench
suite beyond the passes' spread.

## Decision (lane pl8-4): an outer join's ON terms on an earlier table count given its filters

**Decision.** PL.8's estimate (decision 21, CERT). An outer join's ON terms that read only an
earlier table (lane pl8's `on_elsewhere`) were run over that table's sample apart from its filters,
so a pair correlated on the sample counted as independent. Their share is now the earlier table's
sample run with its filters and them together, over its filters alone (`cardinality.rs`,
`JoinedSetRows::new`): the share of the rows its filters keep that the ON terms match, as the inner
join reads the same terms together on the same sample.

**Evidence.** `crates/engine/tests/sample_filters.rs`'s
`an_outer_join_with_sampled_terms_is_estimated_at_least_its_inner_join`: `t0 AS a0 RIGHT JOIN t0 AS
a1 ON NOT (a1.c0 GLOB 'a%') WHERE a1.c1 >= 3` was estimated at 0.684 against its inner join's 1 on
main; it passes with the change.

**Falsified by.** A CERT finding where an outer join's ON terms read only an earlier table.
## Decision (lane pl8-5): a hash join's build side can be a run of tables after the loops

**Decision.** PL.8, the emitter's half of bushy plans. A hash join's operation names a build group
(`HashJoinOp::build_group`, `plan.rs`): the tables of a bushy plan's run `data[start..end]`
(`JoinN::inner_joins`). Its materialized build input (`emitter/select.rs`,
`materialization_prefix`) joins the group's tables among themselves, where a left-deep plan's joins
the tables before the probe; the loops before the probe outside the group stay loops of the main
plan, and the probe reads the group's rows from the payload, so a term between the group and those
loops runs at the probe (`loop_index_for_table_row`). The emitter runs a bushy plan whose runs stand
apart, each followed by an inner hash join built from the run's last table, with no table of the run
probing a hash table built outside it or from a materialized input, no join in it or at the probe
that keeps its place (outer, semi, anti, lateral), no RIGHT or FULL JOIN in the query, and no sort
removed by the plan's order (`optimizer/mod.rs`, `bushy_build_groups`); any other bushy plan is a
typed error naming its tree. For inner joins the run joined among itself and then probed is the
same rows as the left-deep order of the same tables (associativity and commutativity of inner joins,
f25).

**Evidence.** `optimizer::bushy_test_hook::tests::t_pl_8_2_a_bushy_build_group_answers_as_the_left_deep_plan`
(T-PL.8.2): eight joins of four tables (a range join before the run, a WHERE term between the run
and the loop before it, an aggregate, a probe term reading the loop before the run), the reference
plan chosen by cost and with hash joins only. The test builds the bushy plan itself: a hook pins the
plan to the FROM order, loops but for one hash join at the last table built from the table before
it (`bushy_test_hook::PIN`), then splits it into the tree `(a ((b c) p))`. For every query the hook
reports the plan made bushy, and the pinned left-deep plan and the bushy one give the reference
plan's rows. (The search alone put each hash join at the second table, so the earlier test's split
never applied.)

**Not yet built.** The search does not make bushy plans: DPccp's connected complement pairs on an
explicit stack with a budget derived from the plan-time target, falling back to today's search; and
semi and anti join tables placed before other loops. The probe is read whole, as a hash join's
probe is today; a probe that keeps its own access method would let the run's result join a loop
read by a key.

**Falsified by.** An answer of a plan the hook splits that differs from its left-deep plan's; a CERT
finding where an outer join's ON terms read only an earlier table.

## Decision (lane pl8-9): the search makes bushy plans where they cost less

**Decision.** PL.8, the search's half of bushy plans (lanes pl8-7 to pl8-9). After the left-deep
search, `compute_best_join_order_with_context` (`optimizer/join.rs`) pairs each connected subset it
planned (the run) with a table joined to it (the probe) and the connected rest of the tables (the
loops): DPccp's connected complement pairs (Moerkotte and Neumann, VLDB 2006), in the one shape the
emitter runs (decision pl8-5). The loops are the rest's best plan; the run is its plan ending at the
table the probe joins; the probe is an inner hash join built once from the run's rows, materialized,
and read once for each row of the loops. Its cost is the loops' plus the run's plus the hash join's
(`estimate_hash_join_cost`, the probe table's scan at its own pages, PL.7) plus a row's visit for
each row of the run materialized, the terms a left-deep plan's materialized build input pays. The
bushy plan replaces the left-deep one only when it costs less. The pairs are walked on loops over
the subsets found, not recursion (CLAUDE.md section 11), and each pair counts against what the
subsets left of `JOIN_SEARCH_BUDGET` (decision 19): past it the left-deep plan stands. No bushy plan
is tried for a join with an outer join, a RIGHT or FULL JOIN, or a table that must follow another
(`bushy_core`; semi and anti joins after the inner ones follow a bushy plan, decision pl8-15), and a
bushy plan keeps no order, so it removes no sort.

**Why.** A left-deep order joins one table at a time, so two selective joins whose small results
meet only through each other force it to join one side's small result to the other side's whole
table first and carry that large result to the last filter. A bushy plan joins each side on its own
and meets their small results. The data where this holds is the test's below: two fact tables, each
cut down by a filtered dimension, joined on a key of two values.

**Evidence.** `optimizer::bushy_search_tests::tests::t_pl_8_2_the_search_picks_the_bushy_plan_where_it_is_cheapest`
(T-PL.8.2): three queries over that data (rows, and two aggregates, with one or several rows of the
second dimension). Each is planned bushy by the search alone (the planner counts each plan with a
build group it hands the emitter, `BUSHY_PLANS`), and each answers as the plan of nested loops does.
The learned hash probe weight rises about tenfold once the test's first statements have run (a debug
build's hash join timed against a row's visit), so the data makes the left-deep orders' intermediate
result large enough that the bushy plan wins at either weight. The hand-built test of decision pl8-5
(`bushy_test_hook`) stays: it covers shapes the search's data does not (a range join before the run,
a WHERE term between the run and the loop before it, a probe term reading the loop before the run).

**Not yet built.** `JOIN_SEARCH_BUDGET` is still decision 19's 10,000 subsets from Neumann and Radke;
deriving it from a plan-time target (the measured time a candidate takes against the planning time a
query may take) needs that target defined first. The probe is read whole; a probe that keeps its own access method would let the run's result join a loop
read by a key.

**Falsified by.** A bushy plan's answer that differs from the nested loops'; a JOB query whose plan
turns bushy and runs slower than on its parent (AC-PL.8.1).

## Decision (lane pl8-15): weights judged between batches; semi and anti joins after a bushy plan

**Decision.** PL.8, two pieces (lanes pl8-14 and pl8-15).

- *Learned weights* (`optimizer/learned_costs.rs`, T-PL.8.6). A rate's batch (an epoch: as much
  work as calibration timed) replaces the standing rate only when the two differ by more than the
  spread between batch means allows. That spread is the root mean square of successive differences
  of the batch means' logarithms (von Neumann, Kent, Bellinson and Hart, Annals of Mathematical
  Statistics 12(2), 1941); the jump into a batch that moved the rate is left out. The gap over the
  spread is tested against Student's t with as many degrees of freedom as differences, at the
  two-sided 95% level split over the eight kinds of work (Dunn, JASA 56(293), 1961). The test's
  cost stays bounded however long a process runs: the ratio's probability by Abramowitz and Stegun
  26.7.3/26.7.4 while there are few differences, then the quantile's Cornish-Fisher expansion
  (26.7.5) once its last term is under an f64's resolution (about 13,000 differences). The first
  batch the database times replaces calibration's rate or the open's page-read anchor; calibration's
  own error estimate is gone.
- *Semi and anti joins* (`optimizer/join.rs`, `bushy_core`, T-PL.8.2). When a join's tables are
  inner joins free to move followed only by semi and anti joins (EXISTS, NOT EXISTS) that need
  only tables before them, the search pairs the inner joins as decision pl8-9 does, then adds the
  semi and anti joins to each bushy plan as loops in their order, as the left-deep search adds a
  table. The whole plan replaces the left-deep one only if it costs less. A hash join is not
  tried for them: it would build from the run's probe.

**Why.** A burst of load slows a whole batch at once, so the times inside one batch agree however
far the batch sits from the last. Judged by that inner spread (lane pl8-13's ratio estimator), each
burst was evidence: on a loaded adacachy the hash probe's weight climbed in significant steps
through one run (10, 170, 220, 280 rows' visits) and flipped `t_pl_8_2`'s plan from run to run. The
spread between batches holds those bursts, as the method of batch means reads a correlated series'
error (Schmeiser, Operations Research 30(3), 1982). CD-C keeps every table before a semi or anti
join ahead of it and the emitter runs those tables as the last loops (`required_lhs_by_table`), so
a bushy plan of the tables before them is followed by them unchanged.

**Evidence.** `learned_costs::tests::t_pl_8_6_*`: a burst within the spread moves nothing, nor do
two batches alone or one observation alone; a batch past Student's t moves the weight and one just
inside does not; Student's t matches Abramowitz and Stegun's table 26.10; the expansion takes over
only where it agrees with the exact quantile. `bushy_search_tests`: the fact tables hold 12,000 rows,
so the bushy plan is cheapest at every hash-probe weight seen (with 3,000 the nested loops won at a
probe of 400, kiln run r1178; with 12,000 the bushy plan won at every corner, r1271);
`t_pl_8_2_the_bushy_plan_is_cheapest_at_every_corner_of_the_weights` pins those corners (probe 10
and 400 rows' visits, seek and build a quarter of and four times the learned ones; each plan's cost
is linear in the weights, so the corners bound the range);
`t_pl_8_2_semi_and_anti_joins_follow_a_bushy_plan` plans EXISTS and NOT EXISTS, correlated with the
probe's table and with a run's table, bushy and answers as nested loops do. The run of record is
the coordinator's: `t_pl_8_2_the_search_picks_the_bushy_plan_where_it_is_cheapest` passing 20 of
20 copies run at once.

**Not yet built.** Outer joins in a bushy plan: a LEFT JOIN's right table inside the run or after
the probe. Semi and anti joins read by a hash join after a bushy plan. Right after a process
starts, with few batches, Student's t is large, so the weights move slowly until batches gather.

**Falsified by.** A plan of `t_pl_8_2` that differs between two of the 20 copies run at once; a
weight that moves on a batch within the spread between batch means; a semi or anti join's answer
after a bushy plan that differs from the nested loops'.

## Decision (lane pl12): custom plans, then a generic one, for prepared statements

**Decision.** PL.12 (docs/research/planner.md section 9, rank 12; `crates/engine/plan_cache.rs`).
A prepared statement is planned at prepare without values, its generic plan, as before. While it
is planned, every estimate that would take a parameter's value notes that it asked: an `=` or IN
list on a column whose ANALYZE sample keeps common values (decision 20), and a term run over a
table's sample (decision 21). A statement none asked of keeps its generic plan for good: a plan
made with values would be the same plan, so it pays no planning (a key lookup, a table without
statistics, a column whose values are about equally common). For the others, each run with a value
bound chooses:
- With no custom run measured, a custom plan: the statement parsed and planned again with the run's
  values, which the estimates read as they read literals. The program still reads its parameters
  from their registers, so a plan made for one value answers correctly for any.
- After that, the generic plan once its estimated cost is not above the custom plans' average
  raised by their planning: `generic x T <= average x (T + P)`, with `T` the custom runs' running
  time and `P` their planning time (parse, plan and code), both measured by the statement on the
  run itself (the connection's monotonic clock), summed over its custom runs. This is
  PostgreSQL's `choose_custom_plan` comparison, with its planning cost measured where PostgreSQL
  writes `1000 * cpu_operator_cost * (nrelations + 1) * nrelations`, and without its five runs:
  the comparison is made from the first custom run on. Nothing in it is a constant: every input
  is the statement's own estimate or its own measurement, on the host it runs on, as it runs.
- A statement keeps two plans at most: the generic one, and the current run's custom one.
- A schema change or ANALYZE plans the statement again (the existing reprepare, OP_Expire for
  ANALYZE) and starts its choice again, its earlier measurements being of the old statistics.
- `Connection::set_plan_cache_mode` (PostgreSQL's `plan_cache_mode`): `Auto` (the default),
  `ForceGeneric`, `ForceCustom`, which the bench needs to time each plan. An EXPLAIN with values
  bound shows the plan they give, as SQLite's re-prepare with bound values does.
- Scope: SELECT statements (their plans carry an estimated cost); UPDATE and DELETE keep the
  generic plan. A bound LIMIT keeps decision 17's q-middle: its value is not read here.

**Why.** Audit row 8 (`bench/planner/audit/08-fixed-at-prepare.sql`): `WHERE k = ?` had one plan and
one estimate, 1,000 rows, where k = 1 counts 1,000 and k = 9 none. kiln binds every value.

**Sources.** PostgreSQL `src/backend/utils/cache/plancache.c`, `choose_custom_plan` ("Generate
custom plans until we have done at least 5 (arbitrary)"; generic when its cost is below the custom
plans' average plus planning) and `cached_plan_cost`; PostgreSQL docs, PREPARE and 19.7
`plan_cache_mode`; CockroachDB, Query plan type (f35, A); SQLite `sqlite3_prepare_v2` item 3,
recompiling on a bound value that could change the plan.

**Evidence.** `crates/engine/tests/plan_cache.rs` (T-PL.12.3 to T-PL.12.6) and `plan_cache.rs`'s
unit tests (T-PL.12.1, T-PL.12.2). The TPC-H prepared measurement (AC-PL.12.1) and the suite's
(AC-PL.12.2) are the coordinator's.

**Falsified by.**
- A TPC-H query, prepared and run with qgen's parameters, slower under `Auto` than under
  `ForceGeneric` (AC-PL.12.1).
- The bench suite's prepared statements more than 2% slower than the parent (AC-PL.12.2): their
  planning share kept them custom when it shouldn't.
- A statement switched to generic on its first custom run whose later values would have gained
  more than their planning (the comparison judged on one run): seen as runs under `Auto` slower
  than under `ForceCustom` for a skewed parameter stream.

**For PL.9** (statistics kept as rows change, statements re-planned): it should bump the
connection's prepare generation where it re-plans, which resets each statement's choice
(`Statement::reprepare`); a re-plan that keeps statistics close may want to keep the measurements
instead, which is PL.9's call; `PlanValues` is how an estimate reads a bound value.

## Decision (lane pl9): row counts kept as rows change, statements planned again past their statistics' error

**Decision.** PL.9 (docs/research/planner.md section 9, rank 9; `crates/engine/row_changes.rs`).
- Each table keeps, in memory on its database, its rows inserted less its rows deleted since
  ANALYZE last counted it. An INSERT or DELETE of a main-database table adds its own change count
  (the one `changes()` reports) when it finishes: one atomic add a statement, whatever its rows,
  into a counter found when the statement was planned. The write path takes no lock and no lookup.
- The planner's row count for an analyzed table is ANALYZE's count with those changes
  (`base_row_estimate`), as PostgreSQL scales `reltuples` to the table's present size
  (`estimate_rel_size`). The shares the sketch and the sample give (decisions 20 and 21) stay
  shares: they apply to the count as it is now.
- A statement notes, for each table it counted, the count it planned with and how far that count
  may move before its estimates could tell the move apart from their own noise: the relative
  standard error of the table's statistics. With a sample of `n` rows that is `1 / sqrt(n)` (a
  share's binomial standard error `sqrt(p (1 - p) / n)`, relative to the half the sample resolves
  best); with only HyperLogLog sketches, `1.04 / sqrt(m)` for their `m` registers; with only
  `sqlite_stat1`, every row was counted, `1 / sqrt(rows)`. At each run's start the statement
  compares (atomic loads only); a table grown or shrunk past `1 + error` since plans it again
  through the existing reprepare, one prepare, which also starts PL.12's choice again. Within it,
  nothing is planned. Nothing is picked: `n`, `m` and the rows are the statistics' own sizes.
- ANALYZE sets the counts of the tables it counted back to zero when its statistics load.

**Limits, stated.** The counts are in memory: a database opened again plans on ANALYZE's counts
until its next writes. A statement's change count is added when the statement finishes, so a
transaction rolled back after it leaves its rows counted, and rows a trigger or a foreign key's
action writes, an UPSERT's updates, and an UPDATE (which moves no count) are not counted apart.
`ANALYZE t` sets back every analyzed table's count, not only `t`'s. Each is a staleness measure
off by what the next ANALYZE corrects, never a wrong answer.

**Why.** ANALYZE's count stood still between ANALYZE runs, so a table grown tenfold kept the plan
made for a tenth of its rows until someone ran ANALYZE, and a prepared statement kept its plan
however far its tables moved (CH-benCHmark's tables grow as its transactions run).

**Sources.** PostgreSQL `src/backend/optimizer/util/plancat.c`, `estimate_rel_size` (the tuple
count scaled to the relation's current size); PostgreSQL's `n_mod_since_analyze` and autovacuum's
analyze threshold (a picked `50 + 0.1 x reltuples`, which this replaces with the statistics' own
error); Freitag and Neumann, "Every Row Counts", CIDR 2019 (statistics kept as rows change);
Flajolet et al. 2007 (the sketch's error).

**Evidence.** `crates/engine/tests/row_changes.rs` (T-PL.9.1, T-PL.9.2) and `row_changes.rs`'s unit
tests. CH-benCHmark on the parent and the change (AC-PL.9.1) and the suite's writes (AC-PL.9.2)
are the coordinator's.

**Falsified by.**
- CH-benCHmark's analytical queries no faster than the parent's as its tables grow, ANALYZE on the
  same schedule (AC-PL.9.1).
- The bench suite's writes more than 2% slower than the parent (AC-PL.9.2).
- A statement planned again many times as its table grows slowly, each plan the same: the error
  too tight to say when a plan could change.

## Decision (lane pl13): a nested loop that turns into a hash join at the learned costs' break-even, and a plan made again past its estimate's error

**Decision.** PL.13 (docs/research/planner.md section 9, rank 13; `crates/engine/translate/optimizer/adaptive.rs`).
Two run-time checks, each a pure core the executor drives, each with a fixed few counters:
- **The join switch.** A nested-loop or index join counts its outer rows. It turns into a hash join
  at the first outer row whose count reaches the switch point `B x (1 + hash_build) / (seek - hash_probe)`:
  `B` the inner side's rows, the three weights the learned costs' (decision QT.5,
  `learned_costs.rs`), in rows' visits. That is the row at which the seeks' extra cost over probes
  so far equals the cost of reading and building the inner side. Where a probe costs no less than a
  seek there is no point: the join never switches. A planner that chose the nested loop did so for
  an estimate below the point, so the switch fires only when the rows outgrow the estimate.
  Switching there is the ski-rental rule: whatever the outer side's true size, the join costs less
  than twice the cheaper of the two methods chosen in hindsight. Rows already joined by seeks are
  not joined again: the probes take the rows after them, so the answer is the same rows.
- **Re-optimization at a materialization point.** Where a plan holds an intermediate result whole
  (a hash join's build side, a sorter, a materialized subquery), its rows are counted against the
  estimate. Outside `estimate / (1 + error)` to `estimate x (1 + error)`, `error` the relative
  standard error of the statistics the estimate came from (`1 / sqrt(n)` for a sample of `n`, as
  decision "lane pl9" uses it), the rest of the statement is due to be planned again on the counted
  rows. Within it, the estimate could not be told apart from its own noise, and nothing changes.
- **Memory.** The switch's hash table is held to the hash table's share of the statement's budget
  (`vdbe::hash_table::table_share`): before the switch, by the bytes the statistics say it would
  take (the inner rows, each its index entry's values at `Value`'s in-memory size, a format value);
  after the build, by the table's own spill check, since the estimate can miss. A materialization
  point counts the bytes it holds against its budget. Past either, a typed refusal (`AdaptiveError`)
  or a spilled table freed: the join stays a nested loop, the plan under way goes on, both always
  correct answers.
- Nothing is picked: the point moves with the learned weights and the inner side's rows, the band
  with the statistics' own sample size.

**The executor's shell** (`translate/main_loop/adaptive.rs`, `Connection::set_adaptive_joins`, off
by default until AC-PL.13.1's measurement). An inner join's forward equality seek on a table's own
index, in a query, with statistics and a break-even under the learned weights, is emitted with both
inner loops: ahead of the seek, `JoinSwitchStep` counts the outer row and falls through to the seek,
jumps once (at the point) to the hash table's build over the inner table, keyed on the seek's
columns with the rest of the index entry and the rowid beside them, and from then on to its probe.
A probe's match positions the index cursor on the very entry the seek would have found, so both
loops share one body, and the loop's close reads the hash table's next match once switched
(`JoinSwitchHash`). The table takes its own hash table id: a hash join the plan chose keys its table
by its build table's id. A spilled build is freed and the switch refused (`JoinSwitchBuilt`); the
adaptive join has no grace loop.

**The re-plan.** The switch's build is the materialization point the executor drives: it holds the
whole inner table. `JoinSwitchBuilt` sets its rows against the rows the plan counted the table at
(ANALYZE's with the changes since, decision "lane pl9"). Past the band, the table's row counter is
set to the rows counted, so the statement's planned table has moved: it is prepared again on the
counted rows before its next run (`Statement::statistics_moved`), as is every other statement
planned on the table.

**The in-run hand-off** (`Statement::hand_off`, `Connection::set_adaptive_handoff`, on wherever
adaptive joins are). The rest of the run goes to that second prepare where it can start again
without a row twice: a read-only root statement that has returned no row yet. As such a run starts,
`Statement` opens the hand-off; its first row shuts it. With it open, `JoinSwitchBuilt` stops the
run past the band, and the statement starts it again on the plan made now, the way a schema change
part way through restarts one: its implicit transaction ended, the statement prepared again, the
same bound values. Nothing was written and nothing returned, so the new run's rows are the run's only
rows. In autocommit the implicit transaction it ends must be its own: with another statement on the
connection under way in it, the run goes on as planned. A run is handed off once at most: the new plan is made on the counted rows, so its own build
falls within the band, and were it not, it goes on as planned. A run that has returned rows, or
writes, goes on as planned and the new plan waits for its next run: its answer is right and no row
comes twice.

**A hash join the planner chose** (lane pl13r, `BuildCounted`, `Connection::adaptive_build_checks`).
Where adaptive joins are on, a hash join the plan chose counts its build once it is finished, when the
build holds its table whole: no filter applied while building and no materialized input, the table
with statistics and not spilled. The rows are set against the rows the plan counted the table at, the
same band and the same correction of the table's row counter as the switch's build, and the same
hand-off. A filtered build counts the filter's rows, not the table's: correcting the table from it
would be wrong, so it is not checked until a store for filter estimates' corrections is built. The
sorter's and the materialized subquery's checks are not built.

**Why.** Audit row 13 (`bench/planner/audit/10-no-adaptive.sql`): a nested loop chosen for an
estimate ran on however many rows came, and an estimate wrong by orders of magnitude was never
corrected within the statement.

**Sources.** SQL Server's batch-mode adaptive joins (Microsoft Learn, intelligent query processing
details; f32, A): a hash or nested-loop join chosen at run time within one plan, against a threshold
SQL Server picks, which the learned costs replace here. Perron et al., "How I Learned to Stop
Worrying and Love Re-optimization", ICDE 2019 (f34, A): re-optimizing at materialization points on
observed cardinalities cut JOB's 20 longest queries by 27%. Karlin, Manasse, Rudolph and Sleator,
"Competitive snoopy caching", Algorithmica 3, 1988: the ski-rental rule's bound of 2.

**Evidence.** `adaptive.rs`'s unit tests: answers identical with the switch on and off over 300
random joins with NULL and repeated keys (T-PL.13.1, against the cores' model join); the switch
fires once past the point and never at an estimate below it, its cost under twice the best, refused
over budget (T-PL.13.2); a new plan asked only outside the band, the bytes held bounded (T-PL.13.3).
The engine's own (`crates/engine/tests/adaptive_joins.rs`, with the simulator's
`Connection::pin_cost_weights` so the point holds on any machine): random joins (one key and two,
filters, three tables, an aggregate) give one answer with the switch off and on and by cost, the
model's for the plain join, and every plan whose point came under its outer rows switched
(T-PL.13.1); one join switches at the point its plan states and not one row before; an inner table
counted far past its rows (inserted, counted, rolled back) is counted at its build, planned again
once, and the second plan's point comes sooner with no further re-plan (T-PL.13.3). The same
build in an aggregate's run, which returns no row before its end, hands the run off once with the
hand-off on and switches again on the new plan, none with it off, one answer both ways and the
switch-off one; the plain join, its rows returned before the build, hands nothing off and returns
each row once (T-PL.13.4). JOB's 20 slowest queries against the parent (AC-PL.13.1) are the
coordinator's.

**Filtered builds (lane replan-5).** A hash join's build filtered by terms of its own counts the
share of its table those terms keep, which the table's row counter (PL.9) cannot tell: an UPDATE
moves it without moving the count. Where the plan's estimate ran exactly those terms on the sample
(decision 21), the planner notes the share it used (`PlanValues::filter_selectivity`) and the build
is checked against the table's rows now times that share (`BuildCounted` with a `FilteredBuild`).
The band is the share's binomial relative error, `sqrt((1 - p) / (p n))` for a share `p` of `n`
sample rows, and the table count's, added in quadrature as independent errors are. Past it the
counted share (at least half a row's, the sample's own floor) is stored in the database's
`FilterShares`, keyed by the table and a fingerprint of the terms (order-free, never for terms
reading a parameter), and every plan that read the old share is planned again before its next run,
or at once by the hand-off, as one planned on a moved count is. The planner reads a stored share in
place of the sample's for the same terms. Only with adaptive joins on; counted by
`Connection::adaptive_filter_checks` and `Connection::adaptive_filter_corrections` (each also an
`adaptive_replans`). The store holds its entries weakly, so it is bound by the plans that read them:
a share outlives the last statement planned on it only until its entry is swept. A build whose
terms the sample did not run together (an `=` its column statistics estimate, say) or fed by a
materialized input is not checked. Evidence: `pl_13_a_filtered_builds_share_past_its_estimate_plans_again`
(`crates/engine/tests/adaptive_joins.rs`; T-PL.13.5), a range one row in a hundred passed at
ANALYZE and every row after an UPDATE: one answer with adaptive joins off and on, one share stored,
one re-plan, none on the second run or in a statement prepared on the stored share.

**Sorters (lane replan-5).** An ORDER BY sorter fed by a full scan of one table, with nothing
grouped, aggregated, windowed or made distinct in between, holds the rows of that table its WHERE
terms keep. Once the scan has put them all in, ahead of the sort, `SorterCounted` checks their count
as a build's is: against the table's rows by its counter with no WHERE term, against the share the
plan read for exactly its WHERE terms with them, re-planning, storing and handing off by the same
rules. The sorter counts every row put in, kept by a LIMIT or not, in memory or spilled
(`Sorter::inserted`). Counted by `Connection::adaptive_sort_checks`. Evidence:
`pl_13_a_sorters_input_past_the_estimate_plans_again` (T-PL.13.6): a table counted far past its rows
and a range whose share an UPDATE moved, each one answer in order with adaptive joins off and on,
one re-plan, none on the second run.

**Stored subquery results (lane replan-6).** An uncorrelated FROM-clause subquery stored whole in
an ephemeral table (a CTE read more than once, or one declared `MATERIALIZED`, or a source a
RIGHT or FULL JOIN reads again) holds the rows the planner estimated for it. Once its fill
finishes, inside the block that runs it once, `SubqueryCounted` checks their count against the rows
the plan read for the subquery: its own estimate, `estimated_output_rows`, or a count a fill stored.
The band is the statistics' relative error of each table the subquery reads, in quadrature; a
subquery over anything else (another subquery, a table without statistics) is not checked. The
count is the last row's rowid (the fill numbers its rows 1, 2, ...), read without a scan. Past the
band the count goes to the database's `SubqueryRows`, keyed by the subquery's identity: a
fingerprint of its syntax taken when it is planned, since optimizing and emitting the plan rewrite
its printed form; every reference to one CTE, and every statement writing the same subquery, share
the key; a lateral subquery or one reading a parameter has none. The planner reads the count in
place of the estimate (`base_row_estimate`), and every plan that read the old rows plans again, at
once by the hand-off where the run may start over. Counted by `Connection::adaptive_subquery_checks`.
A subquery stored in its seek index (`DirectMaterializedIndex`) is not checked yet. Evidence:
`t_pl_13_7_a_stored_subquery_result_past_the_estimate_plans_again` (T-PL.13.7): a CTE referenced
twice and a GROUP BY in a materialized CTE, each over a range whose share an UPDATE moved: one
answer with adaptive joins off and on, one re-plan, none on the second run or in a new statement.

**Falsified by.**
- JOB's 20 slowest queries not faster than the parent with the shell in (AC-PL.13.1).
- A filtered build's stored share planned on by a statement whose build then counts past it again,
  run after run: the share's error band too tight for the filter.
- A join that switched and ran slower than the nested loop it left, on the same rows: the learned
  weights wrong for the inner side's access (its pages not cached as the seek weight assumes).
- A statement planned again whose new plan is the old one, many times over: the band too tight to
  say when a plan could change.

## 22. Correlated subqueries unnested by dependent joins taken apart, from a worklist (PL.10)

**Decision.** `optimizer/unnest.rs` keeps its fixed patterns and adds the general forms of Neumann
and Kemper's dependent join, each taken from a worklist with a counted bound, never by recursion
(CLAUDE.md §11, §2):

- **Pulling the correlated predicates up** ("pull predicates up to eliminate correlations", Neumann
  and Kemper's BTW 2015 talk, slide 8): an `EXISTS`, `NOT EXISTS`, `IN` or `NOT IN` conjunct of
  `WHERE` whose subquery reads several tables, or has `DISTINCT` or `ORDER BY`, becomes a semi- or
  anti-join with a derived table: the subquery without its correlated conjuncts, reading no outer
  row, run once; the conjuncts join it, reading its tables through its columns.
- **`NOT IN`** becomes an anti-join whose condition is `x = y OR x IS NULL OR y IS NULL`, the parts a
  NOT NULL column can't meet left out: in `WHERE` a NULL test fails the row as FALSE does (SQLite's
  lang_expr.html, "The IN and NOT IN operators"), and an empty subquery passes every row.
- **A mark join** for an `EXISTS` no semi-join can take (under `OR`, in the `SELECT` list), correlated
  by `=` with one outer table: a left join with the subquery grouped by its correlation columns, and
  the test `key IS NOT NULL`. This is the domain `D` inferred from the equivalent inner attributes
  (slide 18: "Instead of joining with D, we can often infer the attributes from D") with the grouping
  pushed below the join (slide 11: `D ⋈ Γ` is `Γ` over `A ∪ A(D)`), the form the one-value aggregate
  already had.
- **Top-down** (Neumann, arXiv:2412.04294, section 5: "visits each operator at most once, starting
  from dependent joins in top-to-bottom order"): the query's own subqueries are taken first; a
  subquery spliced in brings its own correlated subqueries into the query, and they join the end of
  the list. A derived table's own subqueries are unnested when the planner plans it, after this
  query. The bound is twice the correlated subqueries nested in the query's subqueries: each is
  taken at most once as a join and once as a value.

The derived and grouped forms run the subquery's conditions over every row of its tables, where the
correlated form ran them only on the rows each outer row selects; a spliced subquery's own
subqueries may run on rows in another order. So a form is taken only where nothing it moves can fail
or change between calls (`expression_can_fail_on_input`, nondeterministic functions), and a derived
table's own subqueries may read none of the outer query's tables.

A subquery that reads a table a semi- or anti-join brought in stays nested
(`reads_semi_or_anti_joined_table`): those loops stop after a match in their one table, so a join
hung off that table would be a joined inner query they don't run. T-PL.10.1 found it: `EXISTS` in
`EXISTS`, each on its parent, kept 24 of 46 customers when forced before this rule. A nested level
whose key reaches the outermost table (the depth test's) still unnests.

Not taken yet: a scalar aggregate whose result is an expression of an aggregate that isn't NULL
over no rows, such as TPC-H Q20's `0.5 * total(..)` (`result_on_empty_input` takes a bare aggregate
or one NULL on empty input). Q20's answers are checked; it isn't counted as unnested.

The planner keeps choosing by cost: the written form, the patterns' form (as before PL.10) and,
where the general forms went further, the general one are each searched, and the cheapest kept, at
equal cost the one with fewer subqueries left. A query the patterns served keeps its plan when a
general form would cost more.

**Evidence.** T-PL.10.1 (`crates/engine/tests/unnesting.rs`, run with `--features simulator`): 39
classes, the corpus's (bench/planner/audit/07-unnesting.sql), TPC-H Q2, Q4, Q17, Q20, Q21, Q22 and
each new form with NULLs, loose types, collations, outer joins inside and around, answer the same
nested (`SubqueryUnnestingMode::Disabled`), forced and as planned; each class marked to unnest plans
otherwise when forced (Q20 aside, above); EXISTS nested 1 to 24 deep, each level correlated with its parent and the
outermost query, unnests every level. The measurements (AC-PL.10.1, q21 at SF 10 under PostgreSQL's
1.195e+04 ms; AC-PL.10.2, TPC-DS's ten queries on the parent first) are the batch's.

**Falsified by.** An answer of the oracles (TLP, NoREC, DQP, the differential check) or of
T-PL.10.1 that differs between the forms; TPC-DS's ten queries no faster unnested; a query of the
bench whose plan the general form made costlier than the patterns' (the three-way choice should
make that impossible by estimate; the measurement says whether the estimate holds).

## Decision (lane pl11): degree-sequence upper bounds guard each joined set's estimate

**Decision.** PL.11 (docs/research/planner.md section 9, rank 11). ANALYZE's ordered scan of each
index without a WHERE, which counts its distinct prefixes for `sqlite_stat1`, also keeps each
prefix's degree sequence (the rows each distinct value holds) compressed to two norms as the scan
closes each value's run: the largest degree (ℓ∞) and the sum of squared degrees (ℓ2 squared). ℓ1 is
the table's rows, already counted. They go in `__turso_internal_column_stats` beside decision 20's
sketches, `(tbl, index, 'degrees', data)`, 24 bytes a prefix (a u64 and a u128, little-endian: a
format width), and load into `IndexStat::degrees` (`stats.rs`).

`cardinality.rs`'s `DegreeBound` gives each joined set a proven upper bound: start from one
table's rows, or from two tables joined on one column with the product of their columns' ℓ2 norms
(Cauchy-Schwarz, the (2, 2) Hölder pair), then attach each other inner table by the largest degree
of an index prefix whose every column is in an equivalence class with a table already attached, or
by its rows where none is. Every order gives a bound; the least of those a greedy search finds from
each start is kept. `JoinedSetRows::rows` cuts an estimate above it to it; an estimate below it is
left as it is (the plan step: step 3's 791 underestimates stay).

What the bound will not follow, so as never to claim a bound it hasn't proven:
- an outer join's table (it keeps rows that match nothing): a set holding one has no bound;
- a term whose columns differ in affinity or aren't BINARY, or an index column of another
  collation or an expression: there the index's groups are not the term's;
- a table without an ANALYZE count (a derived table, a table never analyzed).

A semi or anti join's table never adds rows and counts as one. The rowid is one row a value. Rows
added since ANALYZE (PL.9's live count) raise each count and each norm by their number:
‖d + e‖p ≤ ‖d‖p + ‖e‖1.

Of the norms, these three are the ones whose Hölder pairs (1, ∞), (2, 2) and (∞, 1) the kept set
closes; LpBound keeps more and solves a linear program over them. That is the step to take if the
coordinator's JOB and TPC-H counts show the bound too loose to matter.

**Why.** At step 3, every prefix the estimates put above their true count is a plan chosen on rows
that can't exist. An upper bound computed from statistics can't be exceeded by the truth, so a cut
to it only moves an estimate towards the truth. Underestimates, step 3's majority, are untouched.

**Sources.**
- Zhang, Mayer, Khamis, Olteanu, Suciu, "LpBound: Pessimistic Cardinality Estimation Using
  ℓp-Norms of Degree Sequences", SIGMOD 2025 (arXiv:2502.05912): a guaranteed upper bound from ℓp
  norms of degree sequences, more accurate than mainstream estimators on JOB and STATS (f19, A).
- Deeds, Suciu, Balazinska, "SafeBound", SIGMOD 2023: compressed degree sequences as bounds, "By
  never underestimating, this approach allows the optimizer to avoid potentially inefficient plans"
  (f40, C), and its open problem, updates without recomputing the sequences.
- Hölder's inequality, Σ_v d_R(v)·d_S(v) ≤ ‖d_R‖p·‖d_S‖q for 1/p + 1/q = 1: the two-table case.

**Evidence.** `cardinality.rs`'s `degree_bound_tests`:
- `pl11_no_counted_join_exceeds_its_bound` (T-PL.11.1): 400 seeds of two to four random skewed
  tables with NULLs, joined by chains, stars, cycles and repeated terms; every subset's counted
  join at or below its bound, every bound finite.
- `pl11_an_estimate_above_its_bound_is_capped_and_one_below_is_unchanged` (AC-PL.11.1).
- `pl11_the_accumulator_keeps_each_prefixs_degree_norms`.

`crates/engine/tests/column_stats.rs`'s `pl11_analyze_keeps_each_index_prefixs_degree_norms`:
ANALYZE's norms equal a GROUP BY's counts on random data; a partial index keeps none.

**Falsified by.**
- A prefix of any query counted above its bound (the property test, or the coordinator's JOB and
  TPC-H runs: AC-PL.11.1, prefixes estimated above their bound exactly 0).
- An underestimate changed by the cut.

**Open.** PL.9 counts rows inserted less rows deleted. An UPDATE that moves rows between values,
or inserts that deletes cancel, aren't in it, so between ANALYZE runs the bound is proven for the
net change only. Making it hold through every write takes a count of rows written, PL.9's call. A
WITHOUT ROWID table's primary key keeps its norms but the bound doesn't yet read them.

## Decision (lane pl7): cost weights fitted to measured statement times, behind a switch

**Decision.** PL.7 (docs/research/planner.md section 9, rank 7). With the database's switch on
(`Connection::set_fitted_costs`, or `AMPHORA_FITTED_COSTS=1` at open; off by default), the database
times a share of its statements and fits the planner's weights to those times
(`crates/engine/translate/optimizer/cost_fit.rs`). The planner reads them through one hook,
`CostModelParams::with_fitted`, after the learned calibration's (`with_learned`, QT.5): each weight
the fit determines replaces the learned one, in page reads; a weight no sample determines keeps the
learned value. Nothing is picked.

- **Terms.** A timed statement gives the work it counted as it ran: rows its cursors stepped
  through, b-tree seeks, rows added to hash tables, hash lookups, in-memory sort work in rows times
  log2 rows, and pages read from the file. Counted, not estimated: the fit is of the cost model,
  and estimates would fold cardinality errors into the weights.
- **Time.** A statement's time is its steps' time and its waits on IO between them; the caller's
  time between a row and the next step, and a busy handler's sleep, are not its work. Each step's
  timing window holds about one clock read, subtracted.
- **Fit.** Time is regressed on the counts, relative to each statement's time (statement times
  span orders of magnitude): non-negative least squares by Lawson and Hanson's active-set method
  on the normal equations, each sample weighted by Huber's function of its residual against the fit
  as it stood (constant 1.345 scales; scale from the residuals' mean absolute value). Terms whose
  columns the others explain to within the arithmetic's precision, and terms the fit holds at zero
  (the constraint, not a measurement), are not determined, and keep the learned value. Nor does a
  weight the samples can't tell from nothing: the planner reads a fitted weight only when it
  stands above zero by more than its standard error times the same 95% Bonferroni quantile the
  epochs are compared by (T-PL.7.6). Without this, a few samples timed on a loaded host moved
  plans at the margin: `tests/planner.rs` failed one run in seven with the switch on.
- **Memory.** The fit keeps two epochs' normal equations, never samples: fixed-size arrays. An
  epoch closes once it alone determines what the previous one did and holds its length; the length
  doubles while consecutive epochs' fits agree within their standard errors (a 95% test,
  Bonferroni-corrected over the weights compared) and halves when they don't. An epoch that no
  longer sees a term closes once it holds as many samples as the previous one: the term goes back
  to its learned weight.
- **Sampling.** A statement is timed as often as what timing it adds (two clock reads a step,
  measured where the engine runs, and the fit's update, timed) stays within the measurement's own
  spread: `rate = min(1, spread × statement time / timing's time)`, every statement until the fit
  has a spread. Deterministic: a running credit, no random draw.

**Why.** Step 3's JOB costs ranked measured times at a Spearman correlation of 0.20. QT.5's
calibration times each kind of work alone, on a page of keys; a statement's work runs beside the
rest of the engine (cursor bookkeeping, record decoding, the page cache), which no isolated
calibration sees. Fitting to the statements' own times weighs the terms as they cost there, on the
hardware and data in use, and follows them as conditions move.

**Sources.**
- Leis, Gubichev, Mirchev, Boncz, Kemper, Neumann, "How Good Are Query Optimizers, Really?",
  PVLDB 9(3), 2015: on JOB, cost-model errors are dwarfed by cardinality errors; the cost model is
  tuned and judged on true cardinalities (f12).
- Lawson and Hanson, "Solving Least Squares Problems", Prentice-Hall 1974, chapter 23 (NNLS); Bro
  and De Jong, "A fast non-negativity-constrained least squares algorithm", J. Chemometrics 11(5),
  1997 (the normal-equation form). The outer loop's bound, three passes a term, is MATLAB
  `lsqnonneg`'s documented default.
- Huber, "Robust Estimation of a Location Parameter", Ann. Math. Statist. 35(1), 1964; Holland and
  Welsch, "Robust regression using iteratively reweighted least-squares", Comm. Statist. A6(9),
  1977 (1.345: 95% efficiency at the normal).
- Bifet and Gavaldà, "Learning from Time-Changing Data with Adaptive Windowing", SDM 2007 (a
  window that grows while the data hold and shrinks when they move); Dunn, "Multiple Comparisons
  Among Means", JASA 56(293), 1961 (the correction).

**Evidence.** `cost_fit`'s unit tests (T-PL.7.2 to T-PL.7.5): on synthetic samples from known
weights with 5% noise the fit lands within four standard errors and 5% of each; one sample in ten a
tenfold stall still leaves every weight within 5%; terms never sampled have no fitted weight and the
planner's value stands; two hundred thousand samples leave the fit's size unchanged; doubled weights
are followed. The planner suites (`tests/planner.rs`, `records_plans.rs`, `join_methods.rs`) pass
with the switch on and off.

**Falsified by.** The coordinator's JOB run (AC-PL.7.1): the rank correlation of estimated cost
with time, counted rows in place of estimates, no higher with the switch on than off.

**Open.** The other parts of PL.7's row: a table's pages from its file rather than its rows over 50
(T-PL.7.1, the audit's 09-cost-model; `cost.rs`, lane qs5-7's), and the P-error harness
(AC-PL.7.2), the coordinator's. External sorts and hash spills give no term yet: their work is
folded into the others'.

## Decision (lane qs5-9): a hash join's Bloom filter sized from its build's measured keys, chosen by the learned weights

**Decision.** PL.5 (and QT.5, query speed step 5), first part. A hash join's Bloom filter, built
from the build side's keys and probed before each probe row reaches the hash table, is sized when
its build ends, from the distinct keys the build measured, at a false-positive rate the planner
derives from the cost model's weights; whether a join gets a filter at all is the same derivation's
answer. It replaces a filter of fixed size (1,024 keys at 1%, Turso's default, which a large build
filled until it passed nearly everything) and the rule "a filter where the probe side has at least
twice the build's rows".

- **The rate** (`bloom_filter_rate_per_key`, `translate/optimizer/access_method.rs`). For `n` keys at
  rate `p` a Bloom filter holds `n·(-ln p)/ln²2` bits (Broder and Mitzenmacher, "Network
  Applications of Bloom Filters: A Survey", 2004, section 2.1). The probe rows that find no match,
  `misses`, still pay the probe for the share `p` that passes. A filter bit is priced at the hash
  table's own memory: a build row's cost over its bits. The total is least at
  `p = bit_cost·n / (ln²2 · misses · probe_cost)`; the plan carries that over `n`, the rate per key,
  so the runtime multiplies it by the keys it measured. The planner chooses the filter when the
  total with it (every probe row's lookup, the passing misses' probes and the bits) is below the
  misses' probes without it; a join whose probe rows all match gets none.
- **Sizing** (`FilterSizing`, `vdbe/bloom_filter.rs`). While the build runs the filter keeps its
  keys' 64-bit hashes; the first probe seals it: distinct hashes counted, the filter built for them.
  A build without keys gives a filter that rejects every key; a rate of one or more, none.
- **Bound.** The hashes and the filter stay within the hash join's memory budget (the statement's,
  derived, executor decision 8): the hashes grow within it and the filter is dropped past it (every
  probe row then passes, as without one), and the rate is never below what the budget's bits allow,
  `exp(-(m/n)·ln²2)`.
- **Answers.** A filter never rejects a key it was given, a NULL key matches nothing, and a full
  outer join probes none, as before.
- **Open.** The filter's own lookup is priced as one key hash (`hash_cpu_cost`); with the learned
  weights, whose build and probe times include hashing, that price is zero, so a measured
  filter-lookup weight (Filter ops timed like the hash probes of QT.5 step 3a) is next.
  LIP (filters from several dimensions applied to the fact scan in their measured order) and
  predicate transfer across the join graph are not built yet.

## Decision (lane qs5-10): calibration from repeated samples, and the filter's lookup and a WHERE step measured

**Decision.** QT.5 and PL.5, query speed step 5. `learned_costs::calibrate` timed each kind of
work once, so a moment of host load fell on one kind and moved the planner's weights from one run
to the next, flipping plans whose costs sat close (the planner tests that failed now and then
under load). It now samples:

- **Rounds.** One sample of each kind a round (a sample is the kind's work repeated until it
  reaches the slowest kind's single run, as before), so a stretch of load falls on every kind
  alike and their ratios, the weights, stay.
- **Medians.** Each kind's rate is the median of its samples. A preempted sample is one long time:
  it moves a mean by its whole length, a median by one rank.
- **Stopping from the spread** (`median_settled`). Among `n` samples the count below the true
  median is binomial, `n/2` with standard deviation `sqrt(n)/2`, so the samples at ranks
  `n/2 -+ sqrt(n)/2` bracket the median by about one standard error whatever the times'
  distribution (Thompson, Annals of Mathematical Statistics 7(3), 1936). A kind stops once half
  that bracket is within the timer's own cost, a pair of clock reads: no reading resolves finer.
  The spread is read each time the samples double. The rounds stop at a page's keys, as a
  sample's repetitions do, so calibration's time is bounded under any load.
- **The filter's lookup** (`Work::BloomProbe`, `bloom_lookup_cost`). Calibration builds a hash
  join's filter over a page of keys, sized as the planner sizes one for one false positive across
  a page of keys it doesn't hold, and times lookups of such keys, the key's hash included.
  `bloom_filter_rate_per_key` prices each probe row's lookup at that, in place of one key hash
  (which the learned weights had made zero).
- **A WHERE step** (`Work::WhereStep`, `cpu_cost_per_where_step`). Two of the engine's values
  compared, timed per key; the planner's step cost is that weight times the row's, in place of
  0.003. The defaults of both stand only where the clock never moved.
- **Open.** `hash_bytes_per_row` is now the build table's row (lane qs5-13, T-QT.5.7), and the
  `rhs_has_selective_seek` gate is gone (lane qs5-15, T-QT.5.8): a seek kept from the start and a
  hash join are weighed by cost alone. LIP and predicate transfer across star joins (T-QT.5.3),
  sort-merge and prefetched index joins are not built.

## Decision (lane g6s3): learned plan hints, kept for a query class only where its held-out runs beat the cost model

**Decision.** G6 step 3, a prototype behind its own switch (`optimizer/plan_hints.rs`). Bao's
approach over Amphora's planner: a hint set says which operators the planner may choose (hash joins,
nested loops, automatic indexes, sorted fetches), as on/off masks the connection's switches carry
(`HintSet::apply`: `set_join_methods`, `set_automatic_indexes`, new here, and `set_sorted_fetches`);
the cost model still chooses among what a mask allows. Ten masks are obeyable (a join method must be
allowed; nested loops alone build no automatic index). For a query, each mask's plan is prepared,
not run, and read into sixteen features from its `EXPLAIN QUERY PLAN` nodes: a count of each
operator and the planner's own row and cost estimates (Bao's per-node operator and cardinality
features, summed over the plan rather than convolved over its tree).

The model is Bayesian linear regression of a run's log time on those features, in constant memory
(the sufficient statistics: a 16 x 16 matrix, a vector and two sums, whatever the number of runs).
Features and times are centered on the runs' means before the fit, so the intercept is the mean
time rather than a weight the prior shrinks (Bishop §3.1.1); uncentered, the features every plan
shares (a join's row estimates) set the prior's scale and the evidence shrank the differences
between plans to nothing (lane g6s3b found the fixed point stuck there on a test's two plans).
Its two precisions are learned from the runs, not picked: the evidence approximation's fixed point,
one step a run, from the unit-information prior. A training run's mask is chosen by Thompson
sampling from the posterior; its measured time trains the model, every run, none chosen out.

The guard keeps the cost model's plan for each query class until a hint set's held-out runs (runs
that never train the model) beat it: the hinted mean log time plus its standard error below the
unhinted mean less its standard error (the one-standard-error rule, Breiman, Friedman, Olshen and
Stone, "Classification and Regression Trees", 1984, §3.4.3). It drops the hint set when its held-out mean lies above the
unhinted mean plus that standard error, and a dropped set stays dropped. A plan equal to the
unhinted one is no hint. The guard holds as many classes as its caller gives it room for, the least
recently used evicted past it.

`bench/plan_hints` is the harness: queries sorted by name and split in turn into training and held
out, training by sampling, then each held-out query's guard trial and its final plan timed against
the unhinted one, answers against SQLite's.

**Why.** The step: hints learned from measured executions, kept only where they beat the cost model
on held-out queries without regressions. A linear model with an exact Gaussian posterior gives
Thompson sampling without the ensemble Bao trains, in fixed memory; the guard, not the model, is
what bounds a regression.

**Sources.**
- Marcus, Negi, Mao, Tatbul, Alizadeh, Kraska, "Bao: Making Learned Query Optimization Practical",
  SIGMOD 2021 (f8): hint sets as operator masks, a model over plan features, Thompson sampling, its
  training data the executions themselves.
- Agrawal and Goyal, "Thompson Sampling for Contextual Bandits with Linear Payoffs", ICML 2013:
  sampling a linear model's Gaussian posterior.
- MacKay, "Bayesian Interpolation", Neural Computation 1992; Bishop, "Pattern Recognition and
  Machine Learning" §3.5.2: the evidence approximation's precisions.
- Kass and Wasserman, "A Reference Bayesian Test for Nested Hypotheses...", JASA 1995: the
  unit-information prior the fixed point starts from.
- Breiman, Friedman, Olshen and Stone, "Classification and Regression Trees", 1984: the
  one-standard-error rule the guard compares held-out means by.

**Evidence.** `optimizer::plan_hints::tests` (the guard admits then drops a regressing hint,
T-G6.3.2; memory bounded over 20,000 runs and classes, T-G6.3.3; the model learns the faster plan)
and `crates/engine/tests/plan_hints.rs` (every mask's answers equal the unhinted plan's and a
computed nested loop's, the planner obeys each mask, T-G6.3.1). Held-out JOB and corpus times are
the coordinator's (AC-G6.3.1).

**Falsified by.**
- A held-out query more than 10% slower with hints on than with the cost model alone (AC-G6.3.1).
- A hinted plan's answer that differs from SQLite's (T-G6.3.1).
- The model's or the guard's memory growing with the runs past the classes it was given.

## Decision (lane pl7c): a table's pages counted by ANALYZE, else derived from its page and records

**Decision.** PL.7, T-PL.7.1. The planner took every table's pages as its rows over 50, a picked
number, so a table of 8 pages and one of 1,011 with the same rows scanned at one cost. Now:

- **Counted.** ANALYZE walks each table's and index's B-tree once more, as `Count` does
  (`Insn::TreePages`, `BTreeCursor::counted_pages`): every interior and leaf page entered once, no
  record decoded. It keeps the count beside its other statistics, `(tbl, index or NULL, 'pages',
  n)` in `__turso_internal_column_stats`. The planner reads a table's rows a page as its rows over
  its pages, an index's the same, for every read of that table (`CostModelParams::for_table`,
  called where each B-tree table is costed: its access methods, its seeks' scores, a hash join's
  probe).
- **Before ANALYZE counts them.** From the connection's usable page bytes and the table's average
  record as ANALYZE's sample measured it, laid out as the file format lays a table leaf's cells
  (an 8-byte header; each cell a 2-byte pointer, the payload's size and the rowid as varints, and
  the record). With no ANALYZE at all, the record is the fewest bytes the format lets a record of
  the table's columns take (one header byte and one serial type byte a column), a lower bound: the
  rows a page are an upper bound, never a picked number. An index without counted pages keeps its
  width against its table's rows a page, as before.
- **Not counted.** Overflow pages: a scan reads a row's overflow only for the columns past it.
  ANALYZE of one index keeps that index's old count, as it keeps its degrees (PL.11).

**Why.** A read's file pages are the cost model's unit (decision 5); the tree has them exactly, so
an estimate of them is the wrong thing to keep. The walk reads each page once, against ANALYZE's
scans that decode every record.

**Sources.** SQLite file format (https://www.sqlite.org/fileformat2.html), sections 1.3.2 (the
default page size, 4096), 1.6 (page header, cell pointers, cells, varints) and 2.1 (record format);
SQLite's `sqlite3BtreeCount`, the walk `Count` follows.

**Evidence.** `tests/column_stats.rs`: `t_pl_7_1_analyze_keeps_each_trees_real_pages` (the pages
kept equal those the table and its index added to the file, none free) and
`t_pl_7_1_plans_read_the_pages_analyze_counted` (two tables of the same rows, one's records a
thousand bytes: the wide one's full read costs more than ten times the narrow one's, where both
cost the same before); `cost::tests::t_pl_7_1_*`. `tests/planner.rs`'
`a_whole_table_group_by_scans_and_sorts_where_lookups_would_read_pages` rested on the 50: its
20,000 items are 198 real pages, under the smallest cache a connection takes (200), so it now
builds 80,000.
