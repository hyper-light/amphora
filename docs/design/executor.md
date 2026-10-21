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
