# The executor: the work a plan's bytecode does

Amphora's engine runs a plan as bytecode (`crates/engine/vdbe`). Each decision here removes work the
bytecode repeated on every run of a statement, where Amphora's bench suite (`bench --suite`,
AC-T1.2.1) and its profiles (`AMPHORA_PROFILE`) found it, recorded with its sources, its evidence
and what would falsify it (CLAUDE.md §5).

## 1. A statement keeps its small ephemeral tables from one run to the next

**Decision.** When a statement is reset to run again, each ephemeral table it opened (an IN list's
keys, an IN subquery's rows, a materialized subquery's index) that holds at most 16 pages in its
pager's cache is kept with its temp file (`ProgramState::ephemeral_kept`), and the next run's
`OpenEphemeral` empties and reuses it, as a table opened twice in one run already was. A larger
table is dropped as before, and every kept one is dropped when the statement is prepared again or
the cursor closed.

**Why.** Each run built its ephemeral tables from nothing: a temp file, a pager with a cache of its
own, a read and a write transaction, page 1 and a B-tree, then dropped them all at the reset. A
profile of the suite's `in_list_10` (ten rows by primary key in an IN list, fedora, 1 client) found
building and dropping the IN list's table about a third of the statement's time: `op_open_ephemeral`
10%, `op_idx_insert` 9%, the reset dropping the temp file 4%, the transaction and page 1 about 8%.

**Sources.** SQLite keeps no ephemeral table between runs, but its are in-memory B-trees on the
connection's pager (`sqlite3BtreeOpen` with a NULL file name, `src/btree.c`), far cheaper to build
than a pager of their own; reusing the table here costs the same as SQLite's once it is built.

**Evidence.** `crates/engine/tests/ephemeral_reuse.rs`: a prepared IN list run four times with other
keys (overlapping, repeated, absent) returns each run's own keys once each; IN subqueries small
enough to keep and too large to keep, run in turns with other values, count each run's own rows.
With a kept table reused without being emptied (a mutant), both fail, the IN list's second run
returning the first run's keys too.

**Falsified by.** A run that reads a key of a run before; a statement holding more memory between
runs than 16 pages for each ephemeral table it opens.

## 2. `IN (SELECT ...)` answers a row with one search of its list

**Decision.** As a condition (`WHERE`, `ON`, `HAVING`), `x IN (SELECT ...)` is one search of the
subquery's ephemeral index: found keeps the row; a miss, or a NULL on the left, fails it. A
condition fails on NULL as on FALSE, so it never asks whether the list holds a NULL. Where the two
differ (`NOT IN`, or the answer used as a value), a one-column list reads its first entry alone: the
list is an ascending index, whose NULLs sort first, so its first entry is NULL if any is. A row
value's list (`(a, b) IN (SELECT ...)`) is still read through on a miss, as which of its rows are
equal but for NULLs depends on more than its first.

**Why.** On every miss the bytecode rewound the list and read all of it for a NULL, a step for each
entry. The suite's `subquery_in` (`grp = ? AND owner IN (SELECT id FROM owners WHERE region = ?)
LIMIT 50`) misses about nineteen rows in twenty against a list of about 200 owners; its profile
(fedora, 1 client) had 55% of its samples in the list's `Next`.

**Sources.** SQLite's `sqlite3ExprCodeIN` (`src/expr.c`): where its FALSE and NULL destinations are
one it codes the search as a single `OP_NotFound`; otherwise, for a scalar left side, it compares
the list's first row alone ("for a scalar LHS, it is sufficient to check just the first row of the
RHS"), and it reads a row value's list through. The answers themselves: SQLite's table of the IN and
NOT IN operators (`lang_expr.html`, "The IN and NOT IN operators"), ISO/IEC 9075-2's <in predicate>
as `= ANY` over the subquery's rows.

**Evidence.** `crates/engine/tests/in_subquery.rs`: 1,000 probes of a list of 2,000, 990 of them
misses, ran 5,961,023 instructions as a condition (bound 120,000) and 11,919,990 as values (bound
240,000); now both fall within the bound. Every form (a value, a condition, under `OR`, `NOT` and
`CASE`, a parameter or a column on the left, lists empty, without NULLs, with NULLs first, between
and last, row values with NULLs in either place) answers what a model of SQL's three values says,
before the change and after; mutants that drop the left side's NULL test, skip the list's NULL
check or read its last entry instead of its first each fail it. Release binaries on fedora (200,000
items, best of 3 in turns, each run started at load under 2, not recorded runs): `subquery_in` 213
→ 784 statements a second at 1 client and 2,846 → 6,833 at 32; `exists_subquery` and
`left_join_anti` unchanged.

**Falsified by.** An `IN` or `NOT IN` that answers other than SQL's three values; a list kept where
a NULL does not sort first (a descending or `NULLS LAST` index would make the first entry say
nothing).

## 3. LIKE jumps to where the literal after a `%` can match

**Decision.** Matching an ASCII pattern against ASCII text (`like_ascii`, the usual case), the
bytes after each `%` start where the pattern's next byte, a literal, next appears in the text in
either case (`like_literal_from`: `memchr2` on its lower and upper case), not at every position in
turn; a wildcard after the `%`, or the pattern's end, searches as before.

**Why.** The suite's `like_substring` (`body LIKE '%zephyr%' LIMIT 20` on 400 bytes of text, one
row in a thousand matching, so about 20,000 rows read) ran at 68 statements a second
(`bench/records/2026-10-21-fedora-suite-amphora-39549dc.jsonl`) against SQLite's 417 on the same
rows (`2026-10-21-fedora-suite.jsonl`): some 730 ns a row against 120. Every position of every text
was a try of the pattern's first literal, a compare and a branch a byte.

**Sources.** SQLite's `patternCompare` (`src/func.c`): past a `%` followed by an ASCII literal, it
finds the next position holding that character in either case with `strcspn` and tries the rest of
the pattern only there. `memchr2` (the `memchr` crate, already built for Amphora's parser) finds the
first of two bytes with vector compares, as glibc's `strcspn` does.

**Evidence.** `crates/engine/vdbe/value.rs`: `like_ascii` answers as `pattern_compare` (SQLite's
algorithm) for every pattern of up to four of `a`, `B`, `%`, `_` against every text of up to five
of `a`, `b`, `A`, `B`, and for thirteen patterns against texts of 0 to 1,000 bytes (the lengths
around 16, 32 and 64 that vector searches handle apart) holding the word early, late, in the other
case or not at all; with `memchr` searching the pattern's own case alone (a mutant), both fail. Release
binaries on fedora (200,000 items, best of 3 in turns, each run started at load under 2; checks,
not recorded runs): `like_substring` 67 → 222 statements a second at 1 client (p99 16.4 → 4.9 ms)
and 867 → 2,556 at 32 (p99 70.7 → 15.1 ms); `filter_no_index` and `point_text` unchanged. What is
left of SQLite's lead is mostly the table's pages read from the file, a few thousand a statement.

**Falsified by.** A text and pattern where `like_ascii` and `pattern_compare` disagree.

## 4. GROUP BY aggregates each row into its group in a hash table

**Decision.** A GROUP BY whose rows don't arrive in group order (no index gives it) aggregates each
row where it arrives, when its aggregates keep a few values each (COUNT, SUM, TOTAL, AVG, MIN, MAX;
no DISTINCT aggregate, no group_concat or JSON aggregate that grows with its group), no window runs
over its groups, its keys have no custom collation or type comparator, and no bare column sits
beside a MIN or MAX (whose row would choose its value). The row's keys are hashed as the hash join
and DISTINCT hash them (an integer and the equal real alike, text by its collation), its group found
or made, and the group's accumulators loaded into the aggregate registers for the row's steps and
saved back (`vdbe/group_table.rs`; `GroupFind`, `GroupSave`); a new group keeps the values its first
row gave the query's other non-aggregate expressions, as the sorter path keeps its first sorted
row's. After the scan the groups are sorted in the order the GROUP BY's sorter gives keys and
drained in it through the output subroutine a sorted group goes through, HAVING, ORDER BY and LIMIT
included (`GroupSort`, `GroupDrain`). The table holds at most the hash join's budget (64 MiB; none
with `temp_store=memory`): past it, a row of a group the table doesn't hold goes to the sorter as
every row did before, so a group is wholly in one or the other, and the sorter's groups are merged
with the table's by key as both are emitted, so the output keeps the sorter's order. Each run of the
GROUP BY begins with its table emptied (`GroupClear`): a correlated subquery runs it again within a
statement, perhaps after a LIMIT left groups undrained. `EXPLAIN QUERY PLAN` says `USE HASH TABLE
FOR GROUP BY`.

**Why.** Every grouped row went through the sorter: made into a record, sorted with every other row
(spilled to files past the sorter's memory), read back and compared with its neighbor, however few
the groups. The suite's `table_group_by` (`GROUP BY grp`: 20 groups over every row) completed no
statement within its two seconds at ten million rows (fedora, 2026-10-22, a check), and the GROUP BYs
over a twentieth of the table (`group_by_having`, `cte`: some 185,000 owners among 500,000 rows) ran
6 and 4 statements a second.

**Sources.** Graefe, "Query Evaluation Techniques for Large Databases" (ACM Computing Surveys 25(2),
1993), §4.2: aggregation by hashing reads its input once where the groups fit in memory, against
sorting's O(N log N), and overflow sends what doesn't fit to be partitioned or sorted. PostgreSQL's
HashAggregate (`src/backend/executor/nodeAgg.c`): a hash table of groups, each holding its
aggregates' transition states, spilling past `work_mem` since PostgreSQL 13. SQLite groups by sorter
or index alone (`src/select.c`, its GROUP BY coding), the path this replaces where it may.

**Evidence.** `crates/engine/tests/hash_group_by.rs` (the debug build's budget is 32 KiB, so the
larger tables overflow into the sorter): COUNT, SUM, MIN, MAX, TOTAL and a FILTERed COUNT over 2,000
rows in 40 groups and 6,000 rows in 5,000 groups, against an oracle, the groups in key order with
NULL first; ORDER BY the key descending, ORDER BY COUNT(*) DESC after a HAVING with a LIMIT, and a
bare LIMIT, past the budget; two-column keys, keys under NOCASE, and 1 and 1.0 as one group; a
correlated subquery whose GROUP BY runs once an outer row with a LIMIT leaving groups undrained;
the plan's hash table where the conditions hold and its sorter where they don't. Four mutants each
fail the tests written for them: no merge at the sorter's group boundaries, no table emptied at a
run's start, keys hashed as BINARY whatever their collation, and groups sorted ascending whatever
the order asked. `cargo test -p amphora-engine`: 2,766 tests pass, two planner tests now naming the
hash table where they named the sorter.

**Falsified by.** A GROUP BY whose hashed answer differs from its sorted one; a hashed GROUP BY
slower than the sorted one on the suite.

## 5. An ORDER BY with a LIMIT keeps only the rows its reader takes

**Decision.** An ORDER BY whose output a LIMIT cuts tells its sorter where the LIMIT (plus any
OFFSET) is: the register that holds it once evaluated, before the first row arrives (`SorterInsert`'s
`limit_reg`). At the first insert, a count from 1 to 65,536 (`MAX_KEPT`) makes the sorter keep only
that many rows, in a max-heap: a row enters while fewer are kept, or when its key orders before the
worst kept row's (compared from its decoded key, with no copy), which then leaves; the rest are
dropped as they arrive. Each kept row carries its arrival number, so rows that tie keep arrival order,
which is what the stable sort of every row gives them. The sort and the reads after it are the
sorter's own, OFFSET and LIMIT applied as before. A larger count, none, or a negative one (no limit)
sorts every row as before. DISTINCT deduplicates before the insert and a GROUP BY's groups arrive
through its output subroutine, so both keep their top rows the same way.

**Why.** Every ordered row went into the sorter, sorted with every other (spilled to files past the
sorter's memory), to emit a few. The engine had a heap for this (an ephemeral index the old row left
by `Last` and `Delete`), but it was chosen only where a LIMIT's registers already existed when the
ORDER BY's sorter was opened, which the main select path sets up only afterwards: an ordinary
`ORDER BY ... LIMIT` sorted every row. The suite's `order_by_sort` sorts some 500,000 rows at ten
million for its top 20; `group_by_having`, `cte` and `join_group` sort up to 185,000 groups for their
top 10, 5 and 10.

**Sources.** PostgreSQL's bounded heap sort (`src/backend/utils/sort/tuplesort.c`,
`tuplesort_set_bound`; EXPLAIN's "Sort Method: top-N heapsort"), used for a sort a LIMIT bounds, over
a scan's or an aggregate's output alike. SQLite's sorter limit (`src/select.c`, `pushOntoSorter`:
with a LIMIT, past LIMIT+OFFSET rows it deletes the last before each insert).

**Falsified by.** An ordered, limited answer that differs from the whole ordering's rows at its OFFSET;
a limited ORDER BY slower than the whole sort on the suite.

## 6. A subquery runs where it is read

**Decision.** A subquery outside the FROM clause is coded once, out of the line of code, as a
subroutine (`subquery.rs`, `emit_non_from_clause_subquery`): an uncorrelated one's body sits behind
its own `Once`, and a correlated one's runs for the row in hand. Each read calls it before using its
result (`ProgramBuilder::emit_lazy_subquery_call`): a scalar's or a row value's registers, an
EXISTS's, an IN's list, an IN that drives a seek, LIMIT and OFFSET. An uncorrelated subquery's call is
a `Once` and a `Gosub`, so it runs at most once, where control first reads it. A correlated one is
called at each read wherever what it reads of the outer row is there at the read: in WHERE, the select
list and ORDER BY of a query whose rows go straight out, for its loop's row; in an aggregate's or a
GROUP BY's output (the select list, HAVING, ORDER BY), from what the output keeps of the rows (a
group's carried columns; an ungrouped query's first row, as its bare columns); in a window query,
inside the window's source query, which takes the whole expression holding it (`window.rs`); in a
RIGHT or FULL JOIN's row body, which the unmatched rows' pass enters too; and in a write statement's
SET, WHERE and RETURNING, for the row in hand (`update.rs`, `delete.rs`, `insert.rs`), but for a row
value `SET (a, b) = (SELECT ...)`, read a register at a time, which runs once a row. Elsewhere (an
aggregate's arguments, a GROUP BY key, a RIGHT or FULL JOIN's WHERE) it still runs at every row of
its loop. A WHERE term holding a subquery is tested in the first loop,
after the terms that hold none, rather than before the loops (`planner.rs`,
`determine_where_to_eval_term`), but where a RIGHT or FULL JOIN's unmatched rows would skip that loop.

**Why.** Amphora ran each uncorrelated subquery before its loops, read or not, and each correlated one
at every row of its loop. One that no row reaches still ran and raised its error where SQLite answers:
the oracles' finding error-99fb1658e34f78cb (sum's integer overflow under `WHERE 1 OR (SELECT
sum(x) ...)`), and with it the same subquery over an empty table, past a WHERE term no row passes,
in a CASE branch no row takes, or beside `WHERE 0`; and a correlated one in a CASE branch of an
aggregate's output, a GROUP BY's, a window query's, a RIGHT JOIN query's or an UPDATE's SET ran at
every row (`SELECT count(*), CASE WHEN max(a.c0) = 99 THEN (SELECT ... a.c0 ...) END`: SQLite
3|NULL, Amphora integer overflow). Its work was spent for nothing too.

**Sources.** SQLite 3.54.0's `sqlite3CodeSubselect` (`src/expr.c`) codes a subquery where it is first
read, as a subroutine (`OP_BeginSubrtn` ... `OP_Return`) whose body sits behind `OP_Once` unless it
is correlated; a later read is an `OP_Gosub` to it. `sqlite3WhereBegin` (`src/where.c`) tests a WHERE
term before its loops (its False-WHERE-Term-Bypass) only if it reads none of the loops' tables and
`exprIsDeterministic` holds, which fails on any subquery (`sqlite3SelectWalkFail`);
`sqlite3WhereCodeOneLoopStart` (`src/wherecode.c`) codes a term holding a correlated subquery
(`TERM_VARSELECT`) after the loop's other terms. In an aggregate query a reference to the query's own
columns is an aggregate column (`TK_AGG_COLUMN`, `analyzeAggregate`), read from the values kept for
the output, so the output's subqueries run there; a window query's select list is evaluated in the
parent of the subquery `sqlite3WindowRewrite` makes; a RIGHT JOIN's unmatched rows run the row body
in a loop of their own (`sqlite3WhereRightJoinLoop`).

**Evidence.** `crates/engine/tests/oracle_findings.rs`: `an_unread_subquery_never_runs` and
`an_unread_correlated_subquery_never_runs` failed before the change with the subquery's integer
overflow; with it they answer as SQLite 3.54.0 does, both where nothing reads the subquery and where
something does (a seek key, an IN that drives a seek, LIMIT, an aggregate's output, which raise the
error as SQLite does). `a_subquery_read_in_two_places_is_coded_once` (GROUP BY and the select list,
an UPDATE's row value) finds the subquery listed once by EXPLAIN QUERY PLAN and its body coded once,
and `a_triggers_subqueries_run_afresh_at_each_firing` finds a trigger's subqueries run afresh at each
firing. `a_correlated_subquery_no_row_reads_never_runs` failed before the correlated subqueries
moved, in an ungrouped aggregate's output, and answers as SQLite with it in every place: the
aggregate, a GROUP BY's select list and HAVING, a window query (one a LIMIT cuts short too), a RIGHT
JOIN query, a SELECT whose rows go straight out, an UPDATE's SET and a DELETE's WHERE, unread and
read. Release binaries before and after, best of 3 in turns at load under 2 on adas-mac-studio
(Apple M4 Max, 14 cores; `bench/records/2026-10-25-adas-mac-studio-lazy-subqueries.md`): time per
statement within 1% for the suite's `subquery_in` and `exists_subquery` at 1 and 32 clients, and
their median alone unchanged; `exists_subquery`'s median at 32 clients, 1.061 in the best of 3, lay
within its own run-to-run spread in five more rounds (the median run 0.988 after over before). With
the correlated subqueries moved, on adas-mac-studio-2
(`bench/records/2026-10-25-adas-mac-studio-2-corr-subqueries.md`): the suite's `subquery_in`,
`exists_subquery` and `group_correlated` within 1% in time per statement, the same programs before
and after (the last is unnested at the suite's size), and `group_correlated_row`, a correlated
subquery in a grouped query's output, 15.8 times as fast at 1 client and 11.4 at 32, its subquery run
once a group instead of once an input row.

**Falsified by.** A subquery that runs where SQLite's doesn't (an error SQLite doesn't raise); an
uncorrelated one run more than once in one run of the query that holds it, or a correlated one run
twice for one read; a subquery statement of the suite slower than before.

## 7. A row's header is parsed once, a cache hit takes no exclusive lock, and the index's terms run before the row is fetched

**Decision.** Three changes to the work every row costs (QT.3, the query-time plan's step 3).

A B-tree cursor keeps the parse of its current row's record header (`vdbe/row_header.rs`,
`RowHeader`, a field of `BTreeCursor`): each column parsed so far, its serial type and where its
value starts. Column and ColumnRange read a column's place from it and parse the header further
only past the last column parsed (`BTreeCursor::record_payload_with_header`). It keeps at most the
cursor's column count of columns, the number the schema gives the cursor when it opens, so it
never holds more than a row of the table has; a column past that is found by walking on from the
last one kept. It is dropped wherever the cursor's noted payload is dropped (every move, every
write through the cursor, every saved position: `forget_noted_payload`), and whenever another
payload is noted.

A page-cache hit takes the cache's shared lock, not its exclusive one (`PageCache::get_shared`,
`Pager::cache_lookup`), and reads only what the cache's map owns: each page's slot in the map
holds a `Weak` handle to the page beside the pointer to its queue entry, and the page's reference
count, an atomic. A hit upgrades the handle (the entry holds the page until both leave the map,
under the exclusive lock) and adds one to the count, up to its cap of 3, with a load and a store;
no `unsafe` is added for it (CLAUDE.md §12), the shared path reaching no entry. The eviction sweep,
which takes one off, holds the exclusive lock and reads the count from the slot by the entry's key,
one map lookup for each entry it examines; so a hit never races the sweep, and two hits at once on
one page may count as one. A `Weak` leaves the page's count of holders as it was, which is what
`evictable` reads. The one case a lookup changes the cache, a page an aborted read left neither
loaded nor locked, takes the exclusive lock and deletes it, as before.

On a loop over a non-covering index, whose table row a deferred seek fetches only when a column the
index doesn't hold is read, the WHERE terms that read the loop's table only through the index (its
stored columns and the rowid alias, read by `IdxRowId`) are tested first, in the order written,
then the rest (`main_loop/conditions.rs`, `DeferredIndex`). An index entry they drop is never
fetched from the table.

**Why.** Column walked the record header from its first serial type on every call: a select list
or a filter reading columns 6, 4 and 10 of a lineitem row walked 6, 4 and 10 serial types (f15 of
the research). The page cache's exclusive lock was taken on every hit, three or four a B-tree
descent, and a hit only bumps a count, so concurrent readers, which step 6 runs, would serialize on
reads that change nothing in the cache's structure. And the terms of a loop ran in written order: a
term on a table column written before a term on an index column fetched every row in the index's
range. `tests/index_filters.rs` found it before the change: a query over 200 entries of an index on
`(k, x)` fetched 200 table rows written one way and 20 the other.

**Sources.** SQLite's `OP_Column` (`src/vdbe.c`) keeps a cursor's parsed header in `aType` and
`aOffset`, with `nHdrParsed` columns parsed and the header's next byte at `iHdrOffset`, valid while
`pC->cacheStatus` equals `p->cacheCtr`, which every cursor move makes stale (`docs/research/
point-reads.md` §5; research finding f16). SQLite's `sqlite3WhereCodeOneLoopStart`
(`src/wherecode.c`) codes a loop's terms in up to three passes, the first only those an index
covers when the table row is fetched by a deferred seek (`OP_DeferredSeek`). Zhang, Yang, Yue,
Vigfusson and Rashmi, "SIEVE is Simpler than LRU: an Efficient Turn-Key Eviction Algorithm for Web
Caches" (NSDI 2024): a hit marks its object and moves nothing, so a hit needs no lock that the
queue's order needs (f35). PostgreSQL raises a buffer's `usage_count` as it pins it, in the
buffer's own state word, with no lock on the buffer table's mapping beyond a shared one
(`src/backend/storage/buffer/README`, "Buffer Ring Replacement Strategy" and the buffer mapping
lock's shared mode; f34).

**Evidence.** `vdbe/row_header.rs`: 2,000 random records of every serial type, NULLs and long
TEXT and BLOB, with a bound below, at and above their width, read column by column in random order
and in runs from random starts, read what the header walk (`nth_into_register`) reads, past the
record's end too; a corrupt header is refused with the walk's errors. `storage/btree.rs`,
`a_cursor_reads_each_row_it_moves_to_from_its_own_header`: a cursor reads each row's own third
column after next, prev, a seek, an overwrite of its row with another layout, and on a row that
spills to overflow pages. `tests/row_header.rs`: a scan, an index seek's deferred seek and a self
join read every column of 600 random rows (overflow pages, rows older than an `ALTER TABLE ... ADD
COLUMN` reading its default) in random orders as the model holds them. With the parse never
dropped (a mutant: `forget` a no-op), the B-tree test and both of these fail. Page cache, under
shuttle (`RUSTFLAGS="--cfg shuttle"`, 2,000 interleavings each): three readers hitting the same
pages under the shared lock return each its own page and leave each with a count of 1 to 3; two
readers hitting while a writer inserts past the capacity and evicts never lose the page a third
holds, and the cache's lists stay consistent. `tests/index_filters.rs`: the query above fetches 20
table rows whichever order its terms are written in, with its answer the model's. Times: the
change's records on jupiter (AC-QT.3.1 to AC-QT.3.4) are the Amphora agent's, with the parent's,
by `bench/planner/record.sh`; until they are recorded the change is unmeasured.

**Falsified by.** A column read through the parse that differs from the header walk's; a cursor
reading a row's column from another row's header; a held or pinned page evicted, or a hit lost so
that a page used since the sweep last passed is evicted first; a query whose answer changes with its
terms' order beyond what SQLite's own order changes (which error a row raises first); q06 or JOB 5a
and 5b no faster per row than on the parent.

## 8. A scan, its aggregates and a hash join's probe run on every core, in morsels

**Decision.** Behind a switch (`vdbe/morsel.rs`, `ParallelMode`, off by default), a pipeline (a
scan with its filter and its grouped or whole-table aggregates, or a hash join's probe side) runs
morsel by morsel on several threads (QT.6, the query-time plan's step 6; G5 step 3). A dispatcher,
one atomic counter, hands the next morsel (a contiguous run of input rows) to whichever worker
asks, so a slow worker takes fewer morsels. Each worker folds its morsels into its own partial
state; the partials merge once every worker ends. The workers are scoped std threads, the caller's
thread among them, never more than the machine's cores (the same code on every platform). One
memory budget, the statement's, is charged by every worker's new group and every match, and a
charge that would pass it is refused with `MorselError::MemoryBudget`, so the workers together never
hold more than the statement may. A worker that ends without an answer is `MorselError::WorkerLost`.
No loop recurses; each worker's loop ends when the morsel indexes run out.

The worker count and the morsel size are no picked numbers: `choose_degree`, the one hook that
picks serial or parallel, minimizes a cost model whose terms are measured where it runs. With `k`
workers, morsels of `m` rows and `n` rows at `r` each, a run costs `(k - 1) * s` to start workers,
`n * r / k` of work, `(n / m) * d / k` to claim morsels and up to `m * r` waiting on the last one;
for each `k` up to the cores, `m` minimizes the last two (`sqrt(n * d / (k * r))`), and the least
time wins against the serial `n * r`. `s` (a worker started) and `d` (a morsel claimed) are
measured on the machine at start (`MachineCosts::measure`) and again by every parallel run (a
running mean, every sample alike); `r` is measured per statement by a pilot on the caller's thread,
morsels doubling from one row, run until it has taken as long as starting one worker (before then
no parallel plan can win) or the input ends. A statement that ends inside its pilot never starts a
worker.

Answers are the same at every worker count and morsel size: counts and INTEGER sums are exact (128
bits); a REAL sum is kept exact until its one rounding (Shewchuk's non-overlapping partials, as
Python's `math.fsum`), so its value does not depend on the order its terms arrive in; MIN and MAX
break ties by the earliest row; a group's key values are its earliest row's; groups come out in key
order; a join's matches come out in probe-row order, each probe row's in build-row order.

**Why.** Every query ran on one thread of 32 (the plan's "Where step 3's time goes"); TPC-H SF 10's
median is a whole scan of lineitem, a scan rate the plan puts at a tenth of today's, which one core
does not reach (f3, f5). A REAL sum summed in another order can differ in its last bits, and the
plan asks that answers be identical at every worker count or within a stated tolerance; the exact
sum makes them identical (Ada's word on this choice is asked for).

**Sources.** Leis, Boncz, Kemper, Neumann, "Morsel-Driven Parallelism: A NUMA-Aware Query
Evaluation Framework for the Many-Core Age" (SIGMOD 2014): morsels handed out by a dispatcher,
workers running a pipeline to its breaker, partial aggregation per worker then a merge (§3, §4.4;
`docs/research/read-path.md`, "Use every core: morsels"). Shewchuk, "Adaptive Precision
Floating-Point Arithmetic and Fast Robust Geometric Predicates" (Discrete & Computational Geometry
18, 1997): the non-overlapping expansion and its exact sum. DuckDB's degree per pipeline from its
rows (research f44) is the same rule as the model's serial bound.

**Evidence.** `vdbe/morsel.rs`'s tests: 300 random filters, groupings and aggregates over random
tables (NULLs, integers past 2^53, REALs whose order changes their naive sum, text) at 2, 3, 4 and
8 workers with random morsel sizes equal the serial answer value for value (T-QT.6.1); 200 random
one- and two-column equi-joins' parallel matches equal the serial probe's in order and a nested
loop's (T-QT.6.2); the exact sum is order-free (T-QT.6.3); a statement past its budget is refused
at every worker count with the budget never passed (T-QT.6.4); the degree follows the costs
(T-QT.6.5). Times are the coordinator's (AC-QT.6.1 to AC-QT.6.3); until recorded the change is
unmeasured.

**Falsified by.** A parallel answer that differs from the serial one; a morsel lost or run twice; a
statement whose workers together pass its budget; a statement the model sends to more workers that
runs slower than on one, measured class for class.

**Reading the table in morsels.** `vdbe/morsel_scan.rs` gives the pipelines a table's own pages
as their input. A morsel is a run of leaf pages, so the unit the pilot measures and
`choose_degree` sizes is a page. The leaves come from the table's interior pages, read level by
level in key order (no recursion; the walk is refused past the database's page count), each leaf
keeping its parent's divider as its greatest rowid. A worker opens its own cursor through the
statement's pager, so every worker reads the statement's one snapshot; it seeks past the divider
below its first leaf and reads until a rowid passes the divider above its last, so every row is
read by exactly one morsel. A row's position, by which MIN, MAX and a group's key values break
ties, is its rowid's place in rowid order. Evidence: random filters, groupings and aggregates over
tables of many leaves, sparse and negative rowids among them, at 1 to 8 workers and several morsel
sizes equal the VDBE's serial SQL answers (T-QT.6.7); a probe read in leaf morsels equals the
VDBE's join in its order (T-QT.6.8); an INTEGER PRIMARY KEY reads its rowid (T-QT.6.9).

**The VDBE's hook.** With `Connection::set_parallel_mode` on (off by default), a top-level SELECT
whose whole work is one rowid table read whole with its filter and aggregates is emitted as
`Insn::MorselAggregate` and a row loop over `Insn::MorselNext`, ahead of the statement's serial
loop (`translate/parallel.rs`). Only shapes whose answers equal the serial loop's qualify: columns
stored as written (INTEGER, TEXT or BLOB affinity, BINARY collation, no generated columns, a
short row padded with its columns' defaults), terms comparing a column with a literal of its own kind or testing it for NULL,
plain-column GROUP BY keys, COUNT, SUM over INTEGER columns, MIN and MAX (in the copied body, AVG
and TOTAL over integers too), and no HAVING, ORDER BY,
LIMIT, DISTINCT, window or subquery; MVCC and the versioned storage, whose rows are not in the
pager's b-trees, never qualify. When the run meets a value its aggregates don't take (a SUM over
text) it jumps to the serial loop, so the answer never depends on the switch. Groups come out in
their keys' SQLite order, as the serial GROUP BY gives them. The machine's costs are measured by a
connection's first parallel statement and learned by each after. Evidence: random queries answer
the same row for row and in order with the switch off and on at 2, 3, 4 and 8 cores (T-QT.6.10).

**The hash join's probe through the hook.** A SELECT whose work is two rowid tables the planner
joins inner by a hash join, with aggregates over the joined rows, is handed over the same way
(`shape_join`): `Insn::MorselAggregate` then carries the build side (`MorselJoinData`). The
build table is read once on the statement's thread, its own filter terms applied and every kept
row charged to the statement's budget, and its hash table built; each worker then reads its run
of the probe table's leaves with its own cursor, tests the probe's own terms, probes, and folds
each joined row (the probe row's columns, then the build row's) into its partial aggregates
(`vdbe/morsel_scan.rs`, `TableJoinAggregate`). It qualifies when the join keys are equalities of
columns of one affinity, every other term is a filter on one table's columns, and every column
read has INTEGER or TEXT affinity and BINARY collation: two such values that compare equal are
the same value, so the order of a probe row's matches, which in the serial loop is its hash
table's, cannot change a MIN, a MAX or a group's key values. A build side past the statement's
memory hands the statement to its serial loop. Evidence: random inner equi-joins on one or two
columns, with filters on either table and groupings and aggregates over both, answer the same
row for row and in order with the switch off and on at 2, 3, 4 and 8 cores, the planner held to
hash joins (T-QT.6.11).

**Left.** A join's own rows (no aggregates) and joins of more than two tables handed over;
expressions beyond a column against a literal (each worker running its own copy of the program's loop body), a hash table built by all
workers, the page cache's miss path sharded, loom
or shuttle on the dispatcher, and the worker and morsel metrics are the step's next pieces.


### The build side filled together, and a join's rows (QT.6, lane qs6-4)

**The build.** A hash join's table is no longer built on the statement's thread. `TableBuild`
(`vdbe/morsel_scan.rs`) hands the build table's leaves out in morsels like the probe's: each worker
keeps the rows of its leaves that pass the build filter and whose key holds no NULL, and scatters
each row's entry (its key and its place in the worker's run) into one list per partition. Then
`Partitioner::fill` (`vdbe/morsel.rs`) gives each partition to one worker, which fills that
partition's map from every run, runs in leaf order: a key's build rows stay in build order
whatever worker read them, and no lock is taken on the table (Leis et al. 2014, section 4.2, build
in the same two phases). The partition count is not picked: it is the machine's measured core
count when parallel mode is on (one partition per worker that may fill), one when it is off. Both
phases run through `execute`, so the pilot and `choose_degree` pick their workers.

**Bounded memory, typed refusals.** Every kept row (its values and their text or blob bytes) and
every scattered entry (its key's parts and bytes, its place) is charged to the statement's one
`MemoryBudget` as it is made; the fill moves the keys, it does not copy them. A build past the
budget is refused as `MorselError::MemoryBudget`, never a partial table, and the VDBE hook then
runs the statement's serial loop, whose hash join spills. T-QT.6.12 checks the refusal at every
degree.

**A join's rows.** A two-table inner hash join with no aggregates, no GROUP BY, LIMIT or
DISTINCT, whose result columns and ORDER BY keys are columns of either table (INTEGER or TEXT
affinity, BINARY collation, no NULLS FIRST or LAST named), runs on morsels too (`TableJoinRows`):
each worker probes its own leaves and keeps each joined row's projected values (the result
columns, then any ORDER BY key not among them), charged to the budget. With an ORDER BY each
worker sorts its morsel's rows and the runs are merged by a stable merge in leaf order, so rows
tied on the keys keep the probe order; without one the runs are concatenated in leaf order
(probe-row order, a probe row's matches in build order). T-QT.6.13 checks it against the serial
hash join at 2, 3, 4 and 8 stated cores.

**A join's rows under LIMIT** (lane qs6-6). A LIMIT or OFFSET cuts the rows in order, so the hook
takes one only when its rows come out in the serial loop's order to the row: the ORDER BY keys hold
both tables' INTEGER PRIMARY KEYs (as columns or as `rowid`), which tell every joined row apart, so
no tie can be cut differently. Both must be integer literals, read as SQLite reads them (a negative
LIMIT is none, a negative OFFSET is none); LIMIT 0 and any other expression stay serial. The hook's
row loop counts them down as the serial loop does (`IfPos` skips the OFFSET's rows, `DecrJumpZero`
ends at the LIMIT); the workers still produce every joined row, so the LIMIT saves output, not
probing. A LIMIT over an order with ties stays serial. T-QT.6.14 checks it row for row against the
serial loop at 2, 3, 4 and 8 stated cores.

**Workers run a copy of the loop body** (lane qs6-7). A one-table scan with its aggregates no
longer needs its filter and inputs to be ones `Predicate` and `Aggregate` hold: the hook emits a
copy of the serial loop's body (`MorselBody`) as its own range of the program, after its row loop
where the serial loop never reaches, translated by the same `translate_condition_expr` and
`translate_expr` the serial loop uses, with each of the table's columns read from a register (the
resolver's expression cache) rather than a cursor. Each worker (`TableBodyAggregate`) makes its own
`ProgramState` (registers, cursors, program counter) at its first morsel, its registers starting
as the statement's were when the hook ran (the constants the program's start computed, the bound
parameters), and per row writes the row's columns and rowid into their registers and runs the
statement's own opcodes from the body's start until it reaches `keep` (its GROUP BY values and
aggregate inputs are then in the output registers, folded into the worker's partial aggregates)
or `skip`. So any filter or expression the serial loop runs, functions, CASE, LIKE, arithmetic,
runs in parallel, with no second evaluator. An error the body raises, or a step that is not a
plain step, sends the statement to the serial loop, which raises the error its own way; so does a
REAL a SUM would take, since the parallel sum's one exact rounding is not the serial loop's. The
GROUP BY values and MIN and MAX are compared bytewise, so a column of another collation, a
COLLATE, a subquery or a column stored converted (REAL or NUMERIC affinity) stays serial. The
versioned storage's column copy still reads the simple shape's own filter and aggregates, and
declines a body only the bytecode states. T-QT.6.15 checks random filters and inputs against the
serial loop at 1, 2, 3, 4 and 8 stated cores, and that the body ran over every row.

**A scan's rows run through the copy too** (lane qs6-8). A one-table SELECT with no aggregates,
no GROUP BY and no ORDER BY emits the same copy of the loop body with the result columns as its
outputs (`MorselBody::rows`); each worker (`TableBodyRows`) keeps the output rows of the rows that
pass, tagged with its morsel's first leaf, and the runs are joined in leaf order, so the rows come
out as the serial loop gives them. An ORDER BY stays serial: its sorter orders ties its own way.

**Workers stop early under a LIMIT** (lane qs6-8). With a LIMIT, the body carries the rows the
LIMIT and OFFSET take (`want`). A shared count of the rows kept is checked before each morsel is
claimed (`Pipeline::done`), so no morsel is claimed once that many are kept, and a morsel stops
scanning once it alone holds that many. A morsel claimed always runs, and morsels are claimed in
leaf order, so the morsels run are a prefix of the table and the first `want` rows in leaf order
are exactly the serial loop's; the hook's row loop then counts the OFFSET and LIMIT down as it
does for joins. This runs behind its own switch, `Connection::set_parallel_limit`, off by default:
off is the control, and the serial loop answers. T-QT.6.16 checks random filters and result
expressions against the serial loop at 1, 2, 3, 4 and 8 stated cores, with and without a LIMIT;
under a LIMIT, that the row count is the serial one, that every row is in the serial result, that
the control never runs the body, and that the body ran over fewer rows than the table holds (the
workers stopped early) for most statements.

**Joins of any number of tables** (lane qs6-10). An inner join of two or more rowid tables, each
read whole (a scan, the planner's hash join, or a seek in an ephemeral index the statement builds
from the whole table), at least one of them a hash join, runs as a chain of hash tables. The probe
is the first table in the join order that no hash join builds from. Every other table is a link
(`MorselJoinData`), taken in the join order among the tables an equality links to those already in
the chain; each equality of two tables' columns is a key of the link that joins the later of its
two tables, so no join term is left to test, and every other term is a filter on one table. Each
link's table is read and built once by the workers together, as a two-table join's build side is.
Each worker walks a probe row through the chain depth first (`each_joined`, an explicit stack as
deep as the chain, no recursion): the first link's matches in build order, each one's matches in
the next link, and so on; the joined row is the probe row's columns, then each link's. A join with
no equality linking a table to the rest (a cross product) stays serial, and a LIMIT or OFFSET is
taken only when the ORDER BY holds every table's INTEGER PRIMARY KEY. The counter
`morsel_join_links` counts the hash tables built. T-QT.6.17 checks random joins of three and four
tables against the serial loop at 1, 2, 3, 4 and 8 stated cores.

**A join's workers run the copy of the loop body** (lane qs6-12). A join's filters and outputs
are no longer only what `Predicate` and `Aggregate` hold: the hook emits the copy of the serial
loop's body (`MorselBody`) over the joined row, each table's columns read from registers at its
place in the chain (the probe's first, then each link's), the probe's rowid its own and a link's
its INTEGER PRIMARY KEY. The equalities that key the chain stay the hash tables' keys, and a term a
table's own `Predicate` holds is still tested on that table's rows, before the probe or the
build; every other term (functions, CASE, LIKE, a comparison of two tables' columns, an equality of
two kinds) and every output (GROUP BY values, aggregate inputs, result columns, then the ORDER BY
keys) run in the body, per joined row, through the VDBE's own opcodes (`TableJoinAggregate` and
`TableJoinRows` hold the body; a join has no other path). The tie argument that held a join's
columns to INTEGER and TEXT now holds of any value but one: an INTEGER and a REAL can compare equal
and differ, so a REAL in a GROUP BY value, a MIN's or a MAX's input (`MorselBody::no_real`), as in
a SUM's, sends the statement to the serial loop, and the serial loop's order of a probe row's
matches can't change the answer. ORDER BY keys stay plain columns of INTEGER or TEXT affinity, so
the workers' sort is the serial sorter's. T-QT.6.18 checks random joins of two, three and four
tables whose filters and outputs only the body states against the serial loop at 1, 2, 3, 4 and
8 stated cores.

**Outer joins** (lane qs6-12). A LEFT, RIGHT (which the planner turns into a LEFT with the tables
swapped) or FULL join of two rowid tables runs on morsels too, whatever loop the planner chose for
it (it makes no FULL hash join): the probe is the table the join may pad with NULLs, the build the
other, keyed by the ON equalities of columns of one affinity (`MorselOuter`, `TableOuterJoin`).
ON terms decide which rows match and WHERE terms test the joined row, matched or padded, so they
are kept apart: only an ON equality is a key; the body's ON terms (`MorselBody::on`) run first, and
a row that passes them matched; the WHERE terms and the outputs follow. A table's own term is
tested on its rows only where that drops no row the join keeps: in a LEFT join an ON term on the
probe and a WHERE term on the build; in a FULL join, none. The build keeps its rows with a NULL key
(they match nothing, and still appear). Each worker marks the build rows its probe rows matched in
a bitmap of its own, a bit per row charged to the statement's budget; the bitmaps merge by OR as
the partials do, and the build rows no worker marked then run the body in morsels of build rows
(`TableOuterRest`), padded with NULLs for the probe's side, its rowid NULL. A FULL join's probe row
that matched nothing runs padded with NULLs for the build's side, by the worker that read it. Rows
come out as the probe's morsels in leaf order, then the padded build rows in build order, sorted
by a stable merge under an ORDER BY. A LIMIT or OFFSET, a join of more tables with an outer one, a
RIGHT join the planner keeps as one, and semi and anti joins stay serial. T-QT.6.19 checks random
LEFT, RIGHT and FULL joins, ON terms on either table or both beside the keys and WHERE terms that
read the padded side's NULLs, against the serial loop at 1, 2, 3, 4 and 8 stated cores.

**A body of nothing but reads** (lane qs6-13). Running the copied body on every joined row costs
a simple join what the direct fold of lanes qs6-9 and qs6-10 did not: every column of every table
written into registers and the body's code stepped, where the fold read only the columns its
aggregates named. A join's body with no ON or WHERE term left past the hash keys and the tables'
own `Predicate`s, and every output a column or a rowid of the joined row, gets `MorselBody::direct`,
each output's place in the joined row (the very register the body's copy reads); with
`Connection::set_parallel_join_direct` on, a worker copies those values and runs no code, `no_real`
still checked. Off by default (the body runs, as lane qs6-12 left it) until the A/B decides:
T-QT.6.20 times simple joins on 054ace65 (the fold) and here (body, and direct) at 1 to 32 cores,
p50, p99 and p99.9; T-QT.6.21 checks direct against the serial loop. An outer join never copies.

**Outer joins under a LIMIT, and LEFT joins among more tables** (lane qs6-13). An outer join's
rows are cut by a LIMIT or OFFSET as an inner join's are: only when the ORDER BY holds every
table's INTEGER PRIMARY KEY. A padded row's NULL key still tells it apart, since a row is padded at
most once, so the sorted rows are the serial loop's to the row (T-QT.6.22). A LEFT join among
three or more tables (`MorselOuter::LeftLink`) puts its padded table last in the chain of hash
tables and probes with a table it keeps: each joined row of the others is probed in that last
link, the ON terms run on each match (`MorselBody::on`), and a row none of whose matches passed
them, or with none, runs once more padded with NULLs for that link's columns, tested by the WHERE
terms alone. No bitmaps: the rows a LEFT join keeps are the probe's own walk. Only an ON equality
with the padded table keys its link; an equality with it in the WHERE clause, or an ON term
between the tables it keeps, is the body's; an ON term on the padded table alone filters its
build, a WHERE term on it does not (T-QT.6.23).

**Outer joins of any kind among any number of tables** (lane qs6-14). A RIGHT or FULL JOIN among
three or more tables, or more than one outer join in a statement, runs as `MorselOuter::Chain`
(`TableChain`): the tables joined in the FROM clause's order, as SQL states the joins, the first
the probe and each other a link of the chain, keyed by its own ON equalities with the tables before
it and marked with what it keeps (`MorselKeeps`: LEFT the left rows, RIGHT the right rows, FULL
both). A probe row walks the chain depth first, an explicit stack as deep as the chain. At each
link a match runs the link's own ON code (`MorselBody::links_on`, a block per link, over the row
up to it); one that passes goes on to the next link, and a row of the tables before it that no
match passed goes on once padded with NULLs when the link keeps the left rows. A link that keeps
the right rows marks the build rows matched in each worker's own bitmap. The walk runs in phases:
the probe's leaves, then for each link that keeps the right rows, in the chain's order, its build
rows no worker marked, padded with NULLs for the tables before it, walk the rest of the chain. A
link's bitmaps are merged only once every phase that can reach it has run: the probe's, and each
earlier such link's padded rows. Every row at the chain's end runs the WHERE terms and the
outputs. Where terms run: a link's own ON term over its table alone filters its build unless the
link keeps its right rows; its other ON terms run in its ON code (an ON term reading a table after
its link declines). A WHERE term acts early only at the probe or an inner link with no link after
it that keeps its right rows (such a link's padded rows skip the earlier tests, and every row that
reaches it marks its build rows whatever the WHERE terms later say): there a term over one table
filters that table and an equality with an earlier table keys the link. Every other WHERE term runs
over the whole row. The two-table shapes and one LEFT join among more keep their own paths
(T-QT.6.24 for FULL joins among three and four tables, T-QT.6.25 for two or more outer joins).

**Two orientations of a two-table outer join, behind a switch** (lane qs6-15; steps 1 and 2 of
lane qs6-14's plan). `Connection::set_parallel_outer_probe` picks which table a two-table LEFT or
FULL JOIN probes, for statements prepared after: `OuterJoinProbe::Padded` (the default, today's)
probes the table the join pads and marks the kept table's matched rows in a bitmap per worker,
padding the unmarked ones once the bitmaps merge (`TableOuterJoin`); `OuterJoinProbe::Kept` probes
the kept table (a FULL join's left one) through the chain, padding a kept row no match passed
during the walk (`TableChain`; a FULL join's right rows still take the link's bitmap). A LEFT join
among more tables and the chain's joins have the one orientation the chain can state (the kept
tables probe, the padded link pads in the walk), whatever the switch says. What each did is
counted on the connection (`OuterCounters`: rows padded during the walk, rows padded after the
bitmaps merged, the bitmap words the workers held, statements per path) and traced per statement
(target `amphora.morsel`, "outer join on morsels"). Every outer-join test (T-QT.6.19, T-QT.6.22 to
T-QT.6.25) runs at both settings; T-QT.6.19 also checks each setting ran its own path.

**The extremes that tell them apart** (lane qs6-16; step 3 of qs6-14's plan). T-QT.6.26, a harness
in `vdbe/morsel_join_ab.rs`, runs LEFT and FULL joins (counts, a GROUP BY, rows) at both settings
and a ladder of stated cores on four pairs of tables with unique keys, each forcing one
orientation's failure mode: a tiny padded table against a huge kept one (`Padded` hashes and marks
the huge one), a huge padded table against a tiny kept one (`Kept` hashes the huge one), every key
but one matched on each side (the bitmaps' rest pass finds nothing) and one key matched (all of it
padded). Per scenario, query, setting and cores it reports p50, p99 and p99.9, the statements each
path ran and those that fell back to the serial loop, the rows padded each way and the bitmap
words per statement, and the bytes charged to the statement's memory budget, the mean and the most
(`OuterCounters::charged_bytes`, `charged_peak`; a budget is not credited back within its
statement, so a statement's charge bounds what it held at once).

**A table the planner reads by a seek, as a link, behind a switch** (lane qs6-16).
Planned by cost, a join onto a primary key or an indexed column is an index nested loop: the
joined table is read by a seek per row of the tables before it (`Search::RowidEq`, `Search::Seek`),
and the hook declined it, so the commonest join of all ran on the serial loop unless the planner
chose a hash join somewhere in it. `Connection::set_parallel_searched_links` (`SearchedLinks::Off`,
today's, the control) lets `shape_join` and `shape_chain` take such a table past the first as a
link keyed by its equalities with the tables before it, as they already take a seek in an index
the statement builds. It is correct because the hook tests every term, consumed or not, on the
joined row: the rows a seek skips fail the terms it was made from. Two candidates, for the
measurements to decide between: `Hash` reads the table whole into a hash link, which pays
against a large probe and loses against a small one (a probe of a few rows, or a seek on a
constant); `Seek` reads an inner join's link keyed by the table's INTEGER PRIMARY KEY by a seek per
joined row, each worker with a cursor of its own (`SeekLink`, `TableSource::seek_row`; its other
key pairs compared as the hash table compares keys, its own terms tested on the row found), no
table built and nothing charged, as the serial loop reads it; any other searched link is a hash
link there too. `MorselJoinData::searched` marks such a link; `morsel_searched_links` and
`morsel_seek_links` count them (T-QT.6.27). T-QT.6.28, a harness beside T-QT.6.26, forces each
candidate's failure mode on a join onto a primary key: a tiny probe against a huge table (`Hash`
reads it all), a huge probe against a tiny one (`Seek` seeks per row where a small hash table
would do), and equal sizes, with p50, p99 and p99.9 and the links per statement at each setting
and stated cores.

**A table with a DEFAULT on morsels** (lane qs6-17). A row written before ALTER TABLE ADD COLUMN
is shorter than its table, and the serial loop's Column reads the missing column as its default,
folded with the column's affinity (`vdbe/builder.rs`'s `folded_default`, SQLite's
sqlite3ColumnDefault; NULL where there is none or it doesn't fold). `stored` refused every table
with a default, so a table that had ever gained a column ran serially. Now `parallel.rs`'s
`defaults` takes each column's value from the same `folded_default`, into
`MorselAggregateData::defaults` and `MorselJoinData::defaults` (empty when all are NULL, so the
common table pads nothing), and `TableSource` pads a short row with them, on scans and seeks
alike. Left serial: a STRICT table's custom-typed column with a default (the serial loop ENCODEs
it at run time), generated columns, and the versioned storage's column copy of such a table.
T-QT.6.29 checks scans' aggregates, groups and rows and joins with such columns on either side
against the serial loop at 1, 2, 3, 4 and 8 stated cores.

**AVG and TOTAL on morsels** (lane qs6-17). The serial loop sums AVG's and TOTAL's values as
doubles with a compensated (Kahan-Babuska-Neumaier) sum, in row order. Over integers whose
magnitudes sum below 2^53, the integers a double holds exactly (its 53-bit significand, IEEE 754
binary64), every partial sum in any order is exact and the compensation is zero, so the serial
answer is the exact sum, as a double, divided by the count for AVG. The copied body's aggregates
(`body_outputs`, scans and joins alike) now take `Aggregate::Avg` and `Aggregate::Total`: each
worker keeps the exact sum, the sum of magnitudes and the count, merged once. A REAL among the
values goes to the serial loop as a SUM's does (`no_real`), and so does a statement whose
magnitudes reach 2^53 (`MorselError::Unsupported` at the finish). T-QT.6.30 checks them bit for
bit against the serial loop at 1, 2, 3, 4 and 8 stated cores.

**What still runs on the serial loop** (lane qs6-16's audit, updated by lane qs6-17; each an open
part of QT.6):
- *A join's rows under a LIMIT whose ORDER BY doesn't hold every table's INTEGER PRIMARY KEY.* The
  answer is the serial loop's rows exactly, and a probe row's matches come in its hash tables'
  order, not the hook's. Parallel needs an order the hook can reproduce (or a check of any rows of
  the right count, as SQL allows) and the scans' early stop.
- *The merge join* (lane qs5-21): not on this branch; once it is an `Operation`, it is taken as a
  hash link (as above), or run by key ranges cut at the driving table's leaves.
- *Correlated subqueries* (`non_from_clause_subqueries`, `outer_query_refs`, `Exists`, `InSelect`):
  the subquery's code needs cursors of its own in each worker; the unnesting the planner already
  does turns some into joins, which do run on morsels.
- *Window functions* (`plan.window`): need a parallel sort, partitioned by PARTITION BY across the
  workers, and a frame pass per partition.
- *Recursive CTEs*: each round needs the last; a round's scan or join could run on morsels once an
  ephemeral table (the queue) is a morsel source (`stored` takes only b-tree tables with a root
  page).
- *Scans on the versioned storage*: its pager holds no table's rows, so every statement its column
  copy doesn't take (joins, expressions only the body states, rows, a statement in a transaction
  that wrote, a table with no copy yet, or the planner's choice of the rows) runs on the serial
  loop. Needs a morsel source over the row versions at the statement's snapshot, cut by key ranges.
- Also found: a table with a generated column (`stored`); a STRICT table's custom-typed column
  with a DEFAULT, which the serial loop ENCODEs at run time; a table whose short rows read a
  default, on the versioned storage's column copy, which decodes records as written; a scan through an index or a
  rowid range (`Operation::Search` on the probe: a rowid range is a run of leaves); a one-table
  scan's rows under ORDER BY; AVG and TOTAL over REALs or past 2^53, group_concat, DISTINCT and
  FILTER aggregates, HAVING, and
  an ORDER BY the GROUP BY can't take (on an aggregate's value) or a LIMIT over groups; DISTINCT, compound SELECTs, INSERT ... SELECT and
  other statements that don't return rows; joins with no equality of one affinity (a cross
  product, a range join).

**Not yet.** One path for LEFT joins: which orientation wins, or a choice per query from measured
costs, is T-QT.6.26's run of record to say (qs6-14's plan, step 4: the leftover paths removed).

## 9. A table scan decodes each leaf once into column batches, behind a switch

**Decision.** With `Connection::set_vectorized_scans` on (off by default; QT.7, the query-time
plan's step 7, with G5 step 2), a read-only statement's scanned table cursors, those it rewinds or
moves to their last row and never seeks, read their columns from a batch (`vdbe/leaf_batch.rs`).
When a Column or ColumnRange reads a row the cursor's batch doesn't hold, the cells of the leaf
under the cursor, from its cell onward (backward for a descending scan), are decoded together:
each record's header parsed once, only the columns the statement reads (found once as the program
is built, `BatchPlan`), into one vector per column. Every Column on the following rows copies its
value out of the vector. Filters and aggregates read those values through the VDBE's one evaluator
of each expression (CLAUDE.md §13); the record format is unchanged.

A batch holds as many rows as fit in the smaller of the core's second-level cache, read from the
operating system where the engine runs (Linux's sysfs `cache/index*/{level,type,size}`, macOS's
`hw.perflevel0.l2cachesize` or `hw.l2cachesize`), and the statement's memory budget
(`Pager::statement_memory_budget`, QT.5's) shared among its batched cursors. Each row is
charged its own record bytes on the page, an upper bound of its decoded text and blob bytes, plus
one value slot for each column read, so the batch never holds more than its budget. The stated
fallbacks: a record that spills to overflow pages, or a row whose charge alone exceeds the budget,
is read row-at-a-time as before; where the cache size can't be read (Windows, which needs an OS
call this crate makes only through unsafe code, CLAUDE.md §12) every scan reads row-at-a-time.

A batch is kept while its cursor is on the same leaf and no other cursor has come to share its
B-tree since the decode (`BTreeCursor::peers_epoch`, bumped as the pager sets the cursor's
`has_peers`); while a peer is there, the cursor reads row-at-a-time. A statement that writes reads
row-at-a-time.

COUNT(*) over a whole table is answered from its leaves' cell counts without decoding a row, as
before (`Count`, `BTreeCursor::count`).

**Why.** Step 3 made a Column parse each header once a row (decision 7), but each Column still
locates the payload and decodes one value, an interpreter dispatch a value. A scan reading k
columns of n rows does that n·k times; decoded by leaf, the parse and the payload's location are
paid once a row in one tight loop, and the values a filter or aggregate reads sit together in cache.

**Sources.** Boncz, Zukowski and Nes, "MonetDB/X100: Hyper-Pipelining Query Execution" (CIDR
2005): vectors of values that stay in the CPU cache amortize interpretation, with 128 to 8K values
working and about 1,000 best on their hardware (research finding f29). Kersten, Leis, Kemper,
Neumann, Pavlo and Boncz, "Everything You Always Wanted to Know About Compiled and Vectorized
Queries But Were Afraid to Ask" (PVLDB 11(13), 2018): a vector's working set must fit the cache,
under 64 or over 64K values lose, and compiled pipelines win on computation where vectors win on
memory stalls (f30). Both fix the batch by the cache, which is why it is measured here and not
picked.

**Falsified by.** Any answer that differs with the switch on (`vdbe/leaf_batch/tests.rs`,
T-QT.7.1 to T-QT.7.3); a batch holding more than its budget; q06 at SF 1 not faster per row with
the switch on than off (AC-QT.7.1, the coordinator's run with `amphora-bench --planner` targets
`amphora,amphora-vectorized`); or G5 step 2 measuring compiled pipelines faster on the same scans,
in which case they replace this.

## 10. A scan loop's filters and aggregates run over a whole batch by selection

**Decision.** With the switch of decision 9 on, a scan loop (`Rewind`, a body, `Next` back to it)
whose body only reads the scanned cursor's columns, filters them and steps COUNT, SUM, TOTAL, AVG,
MIN or MAX is fused (`vdbe/leaf_batch/fused.rs`, found once as the program is built). When its Next
lands on a row of a decoded batch, the body runs over the rest of the batch at once: each body
instruction gets the set of rows that reach it, a bit per row (a selection vector); a comparison,
IS NULL or IS NOT NULL splits its rows between its jump target and the next instruction, so
ranges, IN lists, AND and OR, which the code generator lowers to comparisons and forward jumps,
need nothing of their own; an aggregate step folds its whole selection into its accumulator. The
cursor is then moved to the last row run, where that many Next steps would have put it, and the
Next runs again. Rows reach each instruction in ascending order, so order-sensitive aggregates (a
float sum, the first of equal minima under a collation) fold as they do row-at-a-time.

SQL's semantics stay in one place (CLAUDE.md §13): a comparison is the comparison opcodes' own
`comparison_jumps` (NULLs, affinity, collations, arrays), a REAL column's affinity is
`apply_real_affinity`, and an aggregate step is the opcode's own `agg_step_fast` with its slow path
`op_agg_step_slow` for what that doesn't take (a first row, text, SUM's exact integer overflow,
which ends the statement with the same error). A row the batch left out (its record spills to
overflow pages) ends the fused run before it and is read row-at-a-time. Any other instruction in
the body (an expression, a result row) leaves the loop row-at-a-time, reading its columns from the
batch as decision 9 does.

A hash GROUP BY's step (decision 4: `GroupFind`, the aggregate steps, `GroupSave`) is one step of
the fused body. It takes its selection row by row in order and runs the opcodes' own
`op_group_find` and `op_group_save` for each, so the group table, its budget and its refusals stay
in one place. Between consecutive rows of one group, their keys identical value for value (then
equal under any collation the GROUP BY compares with), the save and the next find are skipped and
the group's accumulators stay in their registers. When the full table refuses a row's new group,
the run ends before that row: the cursor stops on the row before it, and the refused row runs
row-at-a-time on to the sorter's insert, which no batch runs. Filters before the group step have no
effect, so having judged rows past the end costs nothing; a fused body with a group step may hold
no other aggregate step, nor a jump into the step or the sorter's insert. A ColumnRange (several
columns into consecutive registers, as multi-key GROUP BYs read them) is one vector a column.

**Why.** Decision 9 decodes each leaf once, but the interpreter still dispatches every filter and
aggregate instruction once a row. Run by selection, a body instruction is dispatched once a batch
and loops over its rows, the interpretation cost Boncz et al. amortize over a vector.

**Sources.** Boncz, Zukowski and Nes, CIDR 2005 (selection vectors: a filter's output is the
positions of the rows that pass, read by the next operator; research finding f29); Kersten et al.,
PVLDB 11(13), 2018 (f30).

**Falsified by.** Any answer or error that differs with the switch on (`vdbe/leaf_batch/tests.rs`,
T-QT.7.4 to T-QT.7.7, the group table whole and filling mid-batch); a fused loop running a row the
batch left out or a row the group table refused; or AC-QT.7.1 not met with it.

## 11. Skipping rows: zone maps over leaf runs against cracking, behind one switch

G5 step 4 (kiln plan `01a16b0f46ab717ea1e3c87c7f5e0b92`, AC-G5.4.1, T-G5.4.1; lanes g5s4 and
g5s4b). A filter no index serves (filter_no_index: `qty = ? AND price < ?`, 0.6 a second against CedarDB's
2,608 at ten million rows) reads every row. Two ways to read fewer, built as prototypes behind one
switch (`crates/engine/skipping`, `Skipping::{Off, ZoneMaps, Cracking}`), each a pure core kept
from the write path through one hook (`Skipping::on_write`, one call per row written):

- **Zone maps over leaf runs** (`ZoneMap`): a zone per table leaf, its rowid span and the min and
  max of its integer values, with counts. A filter reads only the rowid ranges whose zone can match.
  A write widens its zone or lowers its counts; it never narrows one, so a stale bound is loose,
  never wrong. A zone's size is its leaf's: `zone_map_of_table` walks the table's B-tree and ends
  a zone where the cursor reaches a leaf's first cell, so the file's page size and its rows' bytes
  set it. Rows appended past the last zone start a new zone once the tail holds as many rows as the
  built leaves held on average (their measured fill); a table built empty grows one tail zone
  until built again.
- **Cracking** (`CrackerColumn`): a (value, rowid) copy of the column that each filter partitions at
  its two bounds, so a repeated filter reads only its piece. Inserts and deletes ripple, one move per
  crack above the value, at the write.

Either summary names candidates, never answers: the scan evaluates the filter on every candidate as
a plain scan does. Only integer values are summarized; any other value makes its zone, or its row,
a candidate for every filter, so SQLite's affinity and mixed-type comparisons stay the scan's.

**Sources.** ClickHouse's `minmax` data-skipping index, one summary per granule of index rows
(`src/Storages/MergeTree/MergeTreeIndexMinMax.cpp`); DuckDB's zone maps, per-segment min/max
statistics checked before a row group is scanned (`src/storage/table/row_group.cpp`,
`CheckZonemap`); Idreos, Kersten and Manegold, "Database Cracking" (CIDR 2007) and "Updating a
Cracked Database" (SIGMOD 2007, the Ripple algorithm); the plan's research f2, f10.

**Evidence.** T-G5.4.1 (`skipping::tests`): 64 seeds of 600 random inserts (appended, into gaps,
below every rowid), deletes and updates over tables of up to 400 rows with NULL-like values, filters
between writes, repeated half the time and reaching both ends of i64. Zone maps: the candidate
ranges hold exactly the plain scan's rows, every live row stays inside its zone after every write,
zones were skipped and appended tails started zones (non-vacuity). Cracking: the candidates hold
exactly the plain scan's rows, every matching entry carries the row's current value, the copy holds
every row once, and repeated filters found their bounds cracked. On a real table: zones built from
its leaves hold every row, larger rows make more zones (one a leaf), and with SQL writes fed to the
hook between scans the candidate ranges, read through the engine, return exactly the plain scan's
rows; a view, a missing column and the rowid's alias are refused. The costs (candidates and time per
scan through the engine, the hook's time per write, the summary's build time and size) are the
harness's (`crates/engine/tests/skipping_bench.rs`: an Amphora file, writes through SQL, zones from
its leaves), run by the coordinator; until recorded the prototypes are unmeasured and neither is chosen.

**Wired (lane g5s4c, T-G5.4.2).** A database keeps its summaries in one registry its connections
share (`skipping::SkipRegistry`, on `Database`), one per table, set by `skipping::set_skipping`
(built holding the write lock, so it holds exactly the newest commit). The Insert and Delete
opcodes call the hook for a watched main-database table, capturing the old row first (as they do
for materialized views), so an update is one `RowWrite::Update`. A summary changes at the write,
before commit, and a delete or update lowers counts (a zone left empty is skipped) or moves a row
in the cracker, so neither kind is safe for an older reader. The rule, for both: a reader may
consult a summary only when it is the open writer, or no writer is open and its WAL snapshot is the
newest commit (`connection_wal_pos` against the shared position); any other reader gets
`SkipRefusal::OlderSnapshot` and scans every row. A rolled-back transaction or statement makes the
summaries that took its writes `Stale` until built again; MVCC is refused (`SkipRefusal::Mvcc`).
`skipping::skipping_candidates` is the scan's question and `skipping_counts` what each summary did.

**The scan (lane g5s4d, T-G5.4.3).** At translation a SELECT's plain forward scan of a rowid
table (no index, no outer join in the query) carries a `SkipHint`: each column's integer bounds
that its WHERE terms put on every result row (`a BETWEEN 10 AND 20`, `a >= 10 AND a < 20`,
`5 < a`, `a = 7`, `a BETWEEN ? AND ?`; a real or text constant, a COLLATE, an IN list or NOT
BETWEEN gives no bound: loose, never wrong). At run time the Rewind asks the registry for the hinted column's
candidates only when a summary is kept for it and the reader may consult it (the rule above); a
refusal is counted (`SkipCounts`) and the scan reads every row. With candidates, a `SkipWalk` on
the cursor seeks past each run of rows they leave out (a zone map's rowid ranges, or the cracker's
rowids folded into runs), keeping its place across an I/O wait; the filter is still evaluated on
each row read, so the answer is the plain scan's. A walking scan runs its body row by row (no
fused batch). The statement's `rows_read` counts only the rows read, and each seek counts as a
table seek. No size is picked: a zone is a table leaf (the page size and the record format set
how many rows it holds), the tail zone grows to the leaves' measured fill, and the cracker's pieces
are cut where filters asked.

### Bounds from parameters (lane skip-usable, T-SKU.1)

Real callers prepare a statement once and bind it on each run (`a BETWEEN ? AND ?`, `a >= :lo`), so
a hint holds terms, not resolved bounds: each `BoundTerm` is a column, an operator and either an
integer constant or a parameter's index (`BoundValue`). The Rewind reads each parameter's bound
value from the run's own bindings as the scan starts (`SkipHint::bounds`) and intersects the terms
then, so one prepared statement skips by whatever it is bound to each time. A parameter bound to
anything but an integer (NULL, a real, a text, a blob) gives its term no bound: loose, never wrong,
the same rule as the summaries' (a bound parameter has no affinity, as a numeric literal has none,
so the comparison it makes is the constant's).

### Choosing a kind by measured cost (lanes skip-3 and skip-4, T-SKU.2)

`SkipKind::Measured` keeps both a zone map and a cracked copy of the column, each fed every write
(`skipping::Chooser`, `choose.rs`), and answers each filter with one of them. The executor times
the scan from asking for its candidates to its end and hands the time back
(`SkipRegistry::record_cost`); a scan stopped early (a LIMIT) hands back nothing. The core reads no
clock. Each kind is tried once; the kind whose latest measured scan was cheaper leads; the other is
measured again once the leader has spent as much as the other's last scan cost, times 2 to the
number of measures in a row that confirmed the leader (the doubling trick, Auer, Cesa-Bianchi,
Freund and Schapire 2002; ski rental's break-even rule, Karlin et al. 1988, for the first trial).
The 2 is the doubling trick's base, not a tuned number. A burst of scans issues one trial, not one
each. Instruments: `ChooserStats` (per kind: filters it answered as leader, trials, costs measured,
latest and total cost; how often the leader changed) and a debug event on `amphora.skipping` with
both costs whenever the leader changes.

The scenarios that force each kind's failure mode (`skipping::tests::chooser`): a scattered column
with narrow filters fails zone maps, and the chooser settles on cracking; a column that follows the
rowid, with heavy write churn and wide filters, fails cracking, and it settles on zone maps. In
both, the other kind is measured a logarithmic number of times, and the chooser's total stays
within twice the better kind alone, plus the kept kind's write upkeep.

**Cost of keeping both.** Both summaries take every write, so the losing kind's write upkeep is paid
for as long as the column is measured; the doubling bound covers scans, not that upkeep. Dropping
the loser once its excess cost passes a rebuild's (ski rental again) would bound it; not built.

### A SQL setting (lane skip-4, T-SKU.3)

`PRAGMA skipping = 'table.column'` keeps a measured summary of the column; `'table.column
zone_maps'`, `'table.column cracking'` and `'table.column off'` pick one kind or drop the summary.
The summary is built when the statement is prepared, as the other connection settings take effect,
and as `skipping::set_skipping` builds it: holding the write lock, refused inside an open
transaction, under MVCC, or for a column the record doesn't hold, with the reason. `PRAGMA skipping`
with no value is refused: the setting is set, not read.

**Not yet.** A walking scan runs its body row by row, not in fused column batches; rows read and
latency against DuckDB and ClickHouse are not measured.

**Overlap.** G11's column copy (`storage/versioned/columns.rs`, lane g11s4's summaries) keeps its
own per-block summaries over the copy, fed by commits; these zone maps summarize the row store's
leaves. If the column copy becomes the scan's source, its summaries replace these.

**Falsified by.** A row a plain scan finds that the candidates miss, or a value outside its zone's
bounds after any write; zone maps skipping nothing on time-ordered columns at the suite's size;
cracking's write cost per row above the scan cost it saves at the suite's mix of reads and writes.
