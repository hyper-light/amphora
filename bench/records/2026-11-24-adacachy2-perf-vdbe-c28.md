# Lane C28 (vdbe), 2026-11-24 12:26:31Z to 13:26:31Z: INSTEAD OF triggers on views

Machine: adacachy2 (runner 00aad8da, Linux, 32 cores, load 10 to 36 from other lanes). Every build debug, 8 cores, `nice -n 10`, under `timeout`. SQLite 3.54.0, check-in be8d059e9a49 (Python's, adacachy's system library); the
documents are 3.54.1's (sqlite-doc-3540100). Goal G2 step 3; AC-T1.1.1, AC-T1.8.6; CLAUDE.md 1, 3. Nothing was timed. Started on main 8d70429d, where `CREATE TRIGGER .. INSTEAD OF` was refused (the oracle's 12,547 refusals).

## 1. Losses first

- **NOT MERGED.** adacachy2 stopped running what kiln accepted at 12:59:35Z (r2269; the lead's word: since 13:01Z, and the Macs' links down), and nothing may be built on adacachy. What was pushed after a9b78bbe has not been compiled:
  main's C26, C27 and B29 merged in (foreign keys, recursive triggers), and the fixes of section 4's right column. `cargo fmt` parses and passes it; nothing else of it is checked. Not run at all: clippy, check-production, the four suites
  (windows_and_triggers, oracle_findings, dml_rows, without_rowid_dml), `tests/instead_of.rs`, the sweep at parallel and with statistics, and the red run on main (read, not run: CREATE TRIGGER refuses there, and all but two of the sweep's databases make one).
- **The last measured sweep is red: 435 of 5,871 questions differ at default settings** (tree a9b78bbe: this lane on main 8d70429d, before the fixes below). Each class is read and explained in section 4; the count after the fixes is not known.
- **Found and left, each of another cause** (no query of a view does it on main either):
  - A view's column named with its database, `UPDATE v SET .. WHERE main.v.k = 1`: "no such column: main.v.k"; SQLite takes it. (SET and RETURNING take `main.v.k`.)
  - A view with a column list over VALUES or over `*` (`CREATE VIEW v(k, a) AS VALUES (1, 2)`, `.. AS SELECT * FROM ..`) keeps its query's own names, so its triggers' `new.k` is "no such column".
  - `ALTER TABLE t RENAME COLUMN a TO aa` where a view passes `a` through by its own name and its trigger reads `new.a`: SQLite refuses ("error in trigger g after rename: no such column: new.a"), Amphora takes it and the trigger's statement is refused after.
  - `changes()` after a statement refused as it ran keeps the statement's before it (SQLite: 0), and a WITHOUT ROWID table's INSERT counts twice in `total_changes()` and moves `last_insert_rowid()`; the sweep doesn't ask these.
  - Words: "no such table: q" for a column of a table that isn't there (SQLite: "no such column: q.a"), and a missing table in a trigger's body without its database (SQLite: "no such table: main.v").
- **Where the documents and sqlite3 3.54.0 differ, the documents are held** (`tests/instead_of.rs`, one test each):
  - An alias in WHERE: SQLite refuses `UPDATE v AS q SET .. WHERE q.k = 1` ("no such column: q.k": `sqlite3MaterializeView` stores the rows under the view's own name); lang_update.html and lang_delete.html give the target an alias.
  - RETURNING where no trigger is of the statement's event but one is of another: SQLite takes the statement, returns its rows and does nothing (`vtabIsReadOnly` looks at the whole list); lang_createview.html: "You cannot DELETE, INSERT, or UPDATE a view", but by the trigger.
  - UPDATE FROM of a view over a compound (`.. UNION ALL .. ORDER BY`): SQLite has the columns the statement doesn't set NULL in old and new; lang_update.html: they "are unmodified".
  - A view's column that is a scalar subquery has no affinity (datatype3.html 3.2: "Otherwise, an expression has no affinity"); SQLite gives it the subquery's column's. Not drawn.
- One assertion's words changed: a trigger's reference to a row it hasn't, or to a column that isn't there in a WHEN, says SQLite's words ("no such column: new.a") for tables too. No test held the old ones.

## 2. What it does (branch perf/vdbe-c28)

- `CREATE TRIGGER .. INSTEAD OF` on a view (main, temp, attached; TEMP triggers on another schema's view) is stored and read again as any trigger; SQLite's refusals: INSTEAD OF on a table, BEFORE and AFTER on a view, no such table (trigger.rs). `DROP VIEW` drops the view's triggers,
  TEMP ones too, from the file and from memory (view.rs; `emit_delete_trigger_rows`, schema.rs, was DROP TABLE's own loop).
- `translate/view_dml.rs`: an INSERT, UPDATE or DELETE whose target is a view. The view's columns are typed as a query of it types them (its plan, not the schema's guess: `extract_view_columns` calls an expression TEXT). Every row the statement is for is stored in an ephemeral table before
  the first trigger runs; a loop over it loads `old` and `new`, fires the triggers (`fire_trigger`, unchanged: WHEN, RAISE, conflict clauses and recursion are what a table's are) and evaluates RETURNING. An UPDATE evaluates SET row by row among the triggers; an UPDATE FROM with its join, one row
  a row of the join. `new` takes the view's column affinities in an UPDATE and none in an INSERT; a REAL column's integer reads as a real. No rowid can be named; `SET rowid` and `INSERT INTO v(rowid)` must be integers and go nowhere. UPSERT is refused. `changes()` is 0, `count_changes` counts.
- `instead_of_triggers` (trigger_exec.rs): TEMP triggers first, `UPDATE OF` by the names SET has.

## 3. Read in SQLite 3.54.0 (asked of it before the code; `update.c`, `delete.c`, `insert.c` read in ~/.cache/research-src/sqlite)

| | SQLite |
|---|---|
| `INSERT INTO v(a, a) VALUES (1, 2)` | the first value; `(rowid, rowid)` the last |
| `INSERT INTO v(rowid) VALUES ('x')`, `UPDATE v SET rowid = NULL` | datatype mismatch; an integer is taken and goes nowhere |
| `UPDATE OF rowid` and `SET rowid = 7` | fires: the names are matched |
| `UPDATE v SET .. FROM o` | fires once a row of the join |
| DELETE and RAISE(IGNORE) | count_changes counts the row; an UPDATE or INSERT ignored isn't counted |
| RETURNING's subquery that reads the view | evaluated a row; any other uncorrelated one once |
| `PRAGMA recursive_triggers = ON`, a trigger that inserts into its own view without end | triggers nested too deep |

## 4. The sweep and what it met

`bench/sqlite_instead_of.py 250 C28` -> `crates/engine/tests/fixtures/instead_of.tsv`: 29 forms (the document's example, every refusal with SQLite's words, INSERT, UPDATE, UPDATE FROM, DELETE, count_changes, affinities, comparisons, UPDATE OF, order, TEMP, RAISE, recursion off and on, views over views, a table's
triggers, what a trigger changes under its statement, conflict clauses, transactions, 16 kinds of view) and 250 drawn databases; rows, refusals' words, the triggers' log, `changes()`, `total_changes()`, `last_insert_rowid()`, integrity_check. Held by
`instead_of_triggers_fire_as_sqlite_fires_them` (tests/windows_and_triggers.rs, four settings) and `tests/instead_of.rs` (5 tests: a file opened again, ALTER TABLE beside a view's trigger, and the three places the documents are held).

| met at | what | the cause | what was done (not compiled after a9b78bbe unless it says so) |
|---|---|---|---|
| edd9a12e (r2219, 254 s) | `DROP VIEW` of a view with triggers never ended | it emits its own epilogue and the statement's comes after, so a hoisted constant runs the program again | nothing hoisted there; compiled, passes at a9b78bbe |
| 5037b27e | a correlated subquery in SET panicked, "No index cursor found for table" | emitted without the old row's registers | emitted with them; compiled, passes at a9b78bbe |
| 5037b27e | RETURNING's uncorrelated subqueries evaluated each row beside one that reads the view | one flag for the statement | a flag a column; compiled, passes at a9b78bbe |
| 5037b27e | count_changes of a DELETE with RAISE(IGNORE) | counted after the triggers | before; compiled, passes at a9b78bbe |
| a9b78bbe, 1 question and 4 after it | `SET a = main.v.b` panicked, "Cursor not found" | the cache has the column without its database | `seed_row` has both names |
| a9b78bbe, 6 databases | a REAL column's integer stayed one in `new` and RETURNING | SQLite's OP_RealAffinity where it reads one | `real_columns` |
| a9b78bbe, 2 views (12 questions) | a column list over VALUES and over `*` | section 1, a query's | drawn around |
| a9b78bbe, 1 view | a scalar subquery's affinity | section 1, the document's | drawn around |
| a9b78bbe, 1 question | "no such table: v" for "no such table: main.v" in a table's trigger | section 1, words | held as a refusal |
| a9b78bbe, most of the rest | RETURNING where no trigger is of the event | section 1, the document's | drawn around: each drawn RETURNING is asked only where SQLite takes it without |
| a9b78bbe, 4 databases | UPDATE FROM of a compound | section 1, the document's | drawn around, held by a test |
| a9b78bbe | changes() after a refusal, total_changes() beside WITHOUT ROWID | section 1 | drawn around |

## 5. Reproduce

```sh
python3 bench/sqlite_instead_of.py 250 C28 > crates/engine/tests/fixtures/instead_of.tsv      # SQLite 3.54.0 in Python
cargo test -p amphora-engine --locked --test windows_and_triggers instead_of
cargo test -p amphora-engine --locked --test instead_of
```

Scratch, never merged: branch diag/vdbe-c28 (`tests/c28_probe.rs`: the sweep at one setting with a trace of every statement; C28_FIXTURE, C28_OUT, C28_SETTING). On adacachy2: ~/.cache/amphora-perf/vdbe-c28-remote, ~/.cache/amphora-perf/vdbe-c28-out, target ~/.cache/amphora-perf-target/vdbe-c28.

Lost time: 4 minutes to the DROP VIEW loop run without a trace; 24 minutes (12:59:35 to 13:24Z) to runs kiln accepted and adacachy2 didn't run (r2269, r2277, r2281, r2284, r2291, r2298) and Mac runs that never started (r2279, r2287, r2293), all stopped by this lane; asking
again was no use and, the lead said, made it worse. One commit (00b8a2d7 to e488e679) didn't compile (an export missing, found by reading).

