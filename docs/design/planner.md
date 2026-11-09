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

Each term is compiled once into a flat program of at most 64 operations (a loop over an explicit
stack, CLAUDE.md §11). Each table's terms run within 1,048,576 steps, an operation on a row each
(§2); a term the budget no longer covers is left to the other estimates.

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
- JOB's median planning time over twice step 3's 1.9 ms (the sample's run, at most 1,048,576 steps
  a table, is the cost to watch).
- A CERT failure in the oracles.
