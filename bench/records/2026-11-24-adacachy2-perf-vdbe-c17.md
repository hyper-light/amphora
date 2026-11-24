# Lane C17 (vdbe), 2026-11-24 10:20:45Z to 11:20:45Z: the three panics of lane C11's oracle (CO-window, CP, DC), triggers on WITHOUT ROWID tables, and a fourth panic their sweep met

Machine: adacachy2 (runner 00aad8da, Linux, 32 cores, load 18 to 47). Every build debug, 4 cores, `nice -n 10`, under `timeout`. SQLite 3.54.0, check-in be8d059e9a49 (Python's sqlite3
module and the shell, adacachy's and adacachy2's system library). Goal G2 step 3; AC-T1.1.1, AC-T1.8.6; CLAUDE.md 1. Nothing was timed.

## 1. Losses first

- **A conflict clause inside a trigger is still not its statement's own.** A constraint that fails in a trigger's statement is taken by the clause of the statement that fired the trigger. Not
  fixed (not one of the three, no panic): a trigger program holds several statements and one `resolve_type` (`vdbe/mod.rs`), and the parent reads its own (`execute.rs`, `op_program`). The
  sweep's cases, default settings:
  - `INSERT OR FAIL INTO u ..` in a trigger: SQLite keeps what was written before the failure, Amphora rolls the statement back (`wide1` drawn 465; seed CP of the first draw, drawn 104).
  - An `INSERT OR IGNORE INTO t .. ON CONFLICT(k) DO UPDATE` whose UPDATE trigger breaks a constraint: SQLite refuses the statement, Amphora skips the trigger's statement (`wide1` drawn 523); under
    OR FAIL it keeps the rows SQLite rolls back (drawn 107).
  - `INSERT OR FAIL` as a trigger's last statement makes an earlier one's ABORT a FAIL (`wide1` drawn 489).
- **The committed sweep draws around that cause and four more** (its generator's head names each): UNIQUE ON CONFLICT beside another constraint, which of two constraints is met first
  (`wide1` drawn 23, 129, 236, 392, 586); NOCASE columns in a trigger's WHEN (the oracles' CX, drawn 144); text inserted in an INTEGER PRIMARY KEY under OR ROLLBACK, where Amphora ends the transaction
  and SQLite doesn't (drawn 13); the order a multi-row UPDATE or DELETE visits rows in where the planner chooses an index (with statistics, drawn 332, 343, 399). Of 900 databases of the unrestricted
  draw (seed wide1) 50 differed, for these causes and the two fixed below.
- **INSTEAD OF triggers don't exist in Amphora** (refused at CREATE), so none is in the sweep.
- WITHOUT ROWID tables with generated columns, custom types, UPDATE FROM or CDC still refuse a mutation, triggers or not (docs/design/without-rowid.md).
- `replace_refuses_unsupported_delete_trigger_and_generated_image_before_writing` (tests/without_rowid_dml.rs) asserted the refusal of a REPLACE on a WITHOUT ROWID table with a DELETE trigger. That refusal was the
  design's (docs/design/without-rowid.md, "user triggers .. retain explicit refusals"), and is what stood between CP and SQLite's answer. The design now says what fires, and the test asserts SQLite's answer (the row replaced, no
  trigger fired: lang_conflict.html) and, as before, the generated column's refusal.
- The sweeps compare a query's rows as a set. A statement SQLite refuses is held as refused, not by its words.

## 2. Reproduced, smallest, at default / parallel / statistics / both (r1803 on main 4b5c78b4: the same at all four; stacks and bytecode from it and the run after it)

```sql
-- CO
CREATE TABLE ev(id INTEGER PRIMARY KEY, at TEXT, v INT);
SELECT id, sum(v) OVER (ORDER BY id), lag(v + 1) OVER (ORDER BY date(at), id), coalesce(lag(v + 1) OVER (ORDER BY date(at), id), 0) FROM ev;
-- CP
CREATE TABLE t(k TEXT PRIMARY KEY, v TEXT) WITHOUT ROWID; CREATE TABLE lg(a);
CREATE TRIGGER g AFTER UPDATE OF v ON t BEGIN INSERT INTO lg VALUES (old.k); END;
INSERT INTO t VALUES ('a', 'x'); UPDATE t SET v = 'b' WHERE k = 'a';
-- DC, two causes
CREATE TABLE o(k TEXT, n INT); CREATE TABLE w(k TEXT); INSERT INTO w VALUES ('a');
SELECT b.n NOT IN (SELECT s.k FROM w AS s WHERE b.k IS '1e3') FROM o AS b FULL JOIN w AS c ON NULL GROUP BY b.n NOT IN (SELECT s.k FROM w AS s WHERE b.k IS '1e3');
CREATE TABLE t(a INT, b INT); CREATE TABLE u(a INT); INSERT INTO t VALUES (1, 200); INSERT INTO u VALUES (1);
SELECT 1 FROM t AS x JOIN u AS y ON (CASE WHEN x.a <= NULL THEN NULL END) JOIN t AS z ON ((x.a, NULL, z.b) BETWEEN (1, 1, x.a) AND (7, 10, 41)) WHERE z.b >= 100;
```

| | panic, where | SQLite 3.54.0 | fixed |
|---|---|---|---|
| CO | `window.rs:3457` (translate): lag/lead arg[0] must be a buffer column reference (first_value and nth_value have their own below it) | 1,5,NULL,0; 2,8,6,6; 3,17,4,4 | the same |
| CP | `trigger_exec.rs:708` (translate): UPDATE should contain some updated columns | t 'a','b'; lg 'a' | the same |
| DC, grouped | `vdbe/mod.rs:1607` from `op_rewind` (execute.rs:1935): cursor id 1 is None | 1 | the same |
| DC, row value | `vdbe/mod.rs:1607` from `op_column_fetch_other` (execute.rs:2530): cursor id 4 is None | 0 rows | the same |

## 3. Traced to the first wrong step

- **CO.** `link_with_window` (planner.rs) keeps one entry a window for each occurrence of a call whose text holds a function it takes for nondeterministic (`date(at)`, `random()`), as SQLite
  evaluates those apart in one window. The layer outside pushes a call of another window down as one column, whatever its text (`rewrite_expr_as_subquery_column`; SQLite's
  `selectWindowRewriteExprCb` does, by `sqlite3ExprCompare`). So the inner layer has two entries and one reader; the second is never rewritten, and the emitter meets its original argument. Fixed
  in `prepare_window_subquery`: a layer's functions are the calls its select list reads. `first_value(random()) OVER w = first_value(random()) OVER w` beside another window is 1, as in SQLite.
  (Beside it, found by the sweep: a JSON aggregate's argument over groups never had its aggregates collected, `misuse of aggregate function max()`: `rewrite_late_window_arg`.)
- **CP.** `check_supported` (without_rowid.rs) asks whether an UPDATE has triggers with no updated columns, and an UPDATE OF trigger can't be asked without them. What it asked for was a refusal of every
  trigger. Both are gone: WITHOUT ROWID tables fire their triggers (`RowTriggers`; docs/design/without-rowid.md, Triggers: what update.c, delete.c and insert.c do, with their names).
- **DC, grouped.** Bytecode: `OpenEphemeral 1` (the select list's IN list) is at 11, inside the left table's loop, whose `Rewind 4` at 10 jumps past it when `o` is empty; the unmatched right rows' pass
  enters the row body by `Gosub 16` at 86 and reads cursor 1 (`Found 1` / `Rewind 1` at 57, 58). `assign_select_subquery_eval_phases` (subquery.rs) gave a RIGHT or FULL JOIN no output phase, so a
  grouped query's select-list subquery ran at its loop. With rows in `o` it read the last left row's list for an unmatched row: a wrong answer, no panic (EXISTS gave NULL for 1). Fixed there:
  it runs in the output phase, from the group's carried columns, as in any other join (decision 6 of docs/design/executor.md says so already, with no exception).
- **DC, row value.** Bytecode: instruction 9 `Column 4 0` reads cursor 4 (z's automatic index, opened at 55) while x's automatic index is built (5 to 42). `autoindex_prefilter` (optimizer/mod.rs) took a
  term whose *constraint* reads no other table; a row-value range bounds its first column alone, and the term's other elements are z's. Fixed there: the term's own columns decide. This is also the oracles' DB
  (`ON ((e.day, f.hour) BETWEEN (1, 10) AND (2, 4))`: 4 rows, Amphora 0), in the sweep's first queries.
- **A fourth panic, met by the triggers sweep** (`wide1` drawn 400, storage/btree.rs:2936, "a rowid past the last cell must be past every cell"; no trigger needed, on main, default):

  ```sql
  CREATE TABLE t(k TEXT, a INT, c INT UNIQUE ON CONFLICT REPLACE, PRIMARY KEY (k, a));
  INSERT INTO t VALUES ('e', 1, 2), ('e', 3, 5), ('d', 3, 1), ('a', 60, 3);
  INSERT INTO t VALUES ('c', 60, 5) ON CONFLICT(k, a) DO NOTHING;   -- rowids now 1, 5, 3, 4 on the page
  UPDATE t SET k = k;                                               -- the panic; a debug assertion, so a release build reads a table out of order
  ```

  An INSERT seeks its place again only where a REPLACE may have deleted a row, and `has_ddl_replace` (insert.rs) is false under an upsert clause, whose other constraints still replace. Fixed there.
- **OR ABORT** written out didn't override a trigger's own OR IGNORE, as every other clause does and SQLite's `orconf` does (`wide1` drawn 113, 270, 360, 508, 543). Fixed (`update_trigger_context`, insert.rs).

## 4. Held by

`crates/engine/tests/windows_and_triggers.rs`, three generated fixtures of SQLite's answers, each query at default, parallel, statistics and both, a panic a failure:

| sweep (generator) | questions | asked | main's translate (0c0a3b6c) | fixed |
|---|---:|---:|---|---|
| windows (`bench/sqlite_windows.py`): every window function and aggregate, 1 to 3 windows, ROWS RANGE GROUPS, EXCLUDE, FILTER, PARTITION BY, named, over joins, groups, DISTINCT, ORDER BY, derived, CTE | 820 | 3,280 | 88 differ, 17 panics | 0 |
| triggers (`bench/sqlite_triggers.py`): every event and time, WHEN, UPDATE OF, RAISE, bodies writing a log, another table and the table itself; INSERT, REPLACE, UPDATE, DELETE, UPSERT, RETURNING under every clause; what the tables hold after each, PRAGMA integrity_check | 4,399 | 17,596 | 2,628 of 17,536 differ, 184 panics (before the fourth panic's case was in) | 0 |
| cursors (`bench/sqlite_unread_cursors.py`): subqueries of grouped and aggregate LEFT, RIGHT and FULL JOINs, row-value ranges across joins | 918 | 3,672 | 148 differ, 5 panics | 0 |

(The panics are counted by distinct question. The fixtures' first questions are each finding's own form.) Wider draws, run once on the fixed tree: windows 2,500 a database (5,020 questions) and cursors 2,000 (6,018), seed wide1:
none differ. Triggers, seed wide2, 900 databases: none differ.

Each part taken out (mutants; r1862 on the merged tree 88eedf4e, r1915 on main 837071af): CO 84 answers differ (17 panics), the late argument's aggregates 4, DC's output phase 132 (3 panics), DC's prefilter 16 (2 panics), the row
not read again after BEFORE triggers 20, OR ABORT 16, the INSERT's seek 12 (1 panic). Not taken out: the key sought again after BEFORE DELETE, `new.rowid`'s refusal and the DO UPDATE's firing, whose cases
are in the sweep. Sixteen window queries of shapes the sweep lacks (a window in ORDER BY past a unique key, in EXISTS, in IN, unread columns), 64 asked on main 837071af: none differ (r1915).

## 5. Checks

Linux, adacachy2, the tree landed (20cf0b73, main 0c0a3b6c merged; r1887): `cargo fmt --all --check` 0; `cargo clippy -p amphora-engine --all-targets --locked -- -D warnings` 0; check-production 0 (10,596 left; expect_used
857 to 856); windows_and_triggers 3 passed (24,548 answers), without_rowid_dml 4, oracle_findings 92, dml_rows 16, upsert_conflict_clauses 1, row_values 4. One commit before the last (88eedf4e, r1858: all but the INSERT's
seek, its case and the ratchet): without_rowid_seek 6, without_rowid_affinity 2, row_value_seek 5, in_subquery 4, join_methods 4, on_terms 5, outer_join_subqueries 3, the same suites above green, and clippy 0. After
the landing, on main 837071af: clippy 0 with the log's own `Checking amphora-engine v0.1.0` (r1901); the library's tests `vdbe::` 403 passed, 4 ignored, `translate::` 298 (22 ignored), `window` 6, `trigger` 14,
`without_rowid` 7, `upsert` 6, none failed; in_subquery 4 (r1902). Not run: the suites that need the `simulator` feature (planner, adaptive_joins, unnesting), the corpus, the gate.

Lost time: five runs refused for room (0 to 3 of 32 cores idle). Two copies of the last check, started by a program of mine that ended on an error, ran beside it for two minutes and were ended by their process groups.

## 6. Reproduce

```sh
python3 bench/sqlite_windows.py 400 CO > crates/engine/tests/fixtures/windows.tsv                  # as committed; 2500 wide1: wider
python3 bench/sqlite_triggers.py 250 CP > crates/engine/tests/fixtures/triggers.tsv                # 900 wide2: wider
python3 bench/sqlite_unread_cursors.py 300 DC > crates/engine/tests/fixtures/unread_cursors.tsv    # 2000 wide1: wider
cargo test -p amphora-engine --locked --test windows_and_triggers
```
