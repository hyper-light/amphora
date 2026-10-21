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
