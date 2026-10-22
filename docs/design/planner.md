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
