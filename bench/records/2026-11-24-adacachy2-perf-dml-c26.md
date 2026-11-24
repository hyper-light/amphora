# Lane C26 (dml), 2026-11-24 12:05:36Z to 13:05:36Z: foreign keys counted as SQLite counts them, recursive_triggers, and the order two passes go in

Machine: adacachy2 (runner 00aad8da, Linux, 32 cores, load 16 to 43). Every build debug, 8 cores (6 for the last), `nice -n 10`, under `timeout`. SQLite 3.54.0 (Python's sqlite3 module).
Goal G2 step 3; AC-T1.1.1, AC-T1.8.6; CLAUDE.md 1, 4, 11. Nothing was timed.

## 1. Losses first

- **Under `PRAGMA recursive_triggers = ON`, REPLACE fires no DELETE trigger for the rows it removes.** SQLite fires them and then checks the constraint again (`sqlite3GenerateRowDelete`, lblRecheckOk). Smallest:
  `CREATE TABLE t(k INT PRIMARY KEY, n INT); INSERT INTO t VALUES (1, 1); CREATE TABLE lg(x); CREATE TRIGGER g AFTER DELETE ON t BEGIN INSERT INTO lg VALUES (old.k); END; PRAGMA recursive_triggers = ON; REPLACE INTO t VALUES (1, 2)`:
  SQLite logs 1, Amphora nothing. 8 of the sweep's forms and 1 drawn database in 250 (`C26_REPLACED=1 python3 bench/sqlite_foreign_keys.py`), left out of the fixture. INSERT's preflight writes index entries as it goes under REPLACE; with a trigger
  between two constraints it must not, as an upsert's doesn't.
- **What a BEFORE DELETE trigger that writes the row it is on leaves** is undefined (lang_createtrigger.html); SQLite judges the key the row had before it (`iOld`), Amphora the one it has after. Left out of the draw; one of 700 wide databases (269 of wide1).
- **DROP TABLE of a parent** still has its own counting (`emit_fk_drop_table_check`): SET DEFAULT and the actions' own violations aren't counted there. Not asked by any sweep.
- **`PRAGMA defer_foreign_keys`** and the order several child tables' actions run in (`resolved_fks_referencing` goes through a hash map; SQLite's newest first) were not looked at.
- **A multi-row UPDATE of a table with several UNIQUE keys and no trigger, foreign key or REPLACE** goes in the order of the index Amphora picks, SQLite in its own (C17's 343). One drawn database in 250 met it without foreign keys.
- Cost, not measured: a DELETE of several rows of a table with a foreign key, or that one references, now keeps its rowids first (SQLite does); a parent's removal scans every child that references it, not the first, and under CASCADE too.
- The engine's library tests were not run. Nor macOS, the corpus, the gate.

## 2. Reproduced (SQLite 3.54.0's rows, errors with words, trigger log; default and parallel the same at each)

`bench/sqlite_foreign_keys.py 250 C26`, the landed fixture, on main a7d4af72 with the test alone (432a7e00): **6,552 of 63,848 answers differ**; asked at default and parallel, 3,156 of 31,924 and 268 in their words alone, in 129 databases.

| part | databases | questions | what differs |
|---|---:|---:|---|
| SET DEFAULT of a key that goes (bug 1) | 12 | 134 | DELETE taken, the orphan kept |
| a violation a program leaves (bug 2) | 10 | 660 | refused where the program ends, or kept under FAIL |
| the order rows go in, WITHOUT ROWID (bug 4) | 17 | 27 | the triggers' log, and what they write |
| a table that references itself | 23 | 517 | rows of a statement that mend each other refused at the first (C25's b); REPLACE; RESTRICT's order |
| rows that mend each other; emp | 3 | 48 | the same, two tables; C25's `INSERT INTO emp VALUES (2, 1), (1, NULL)` |
| FAIL and RAISE(FAIL) beside a broken key | 4 | 26 | C25's a: `UPDATE t SET f = 8` keeps (1, 8) |
| a foreign key's action that fires the trigger it is in | 1 | 3 | two rows of three deleted more |
| drawn | 59 | 342 | each of the above, and composite keys refused as mismatched |

Bug 3 (recursive_triggers) on the same protocol before its commit (tree 3e657b9e): 14 of its ON forms, 80 questions.

## 3. Traced to the first place

- **Bugs 1 and 2, and C25's a, b, c: the count.** SQLite keeps one count a statement (`Vdbe.nFkConstraint`) and one a transaction, and `sqlite3FkCheck` moves them for every row that leaves (`fkScanChildren` +1 a child, `fkLookupParent` -1) and every row that
  comes (+1, -1 a child), whatever the action. Amphora counted one a key under NO ACTION, none under CASCADE, SET NULL and SET DEFAULT, took nothing back for an immediate key, compared keys under REPLACE in its place, and each program a statement ran had a count of its
  own which `halt` checked at that program's end (`halt`, before `is_trigger_subprogram`).
- **The entry under the cursor**: a DELETE by an index deleted that index's entry after the foreign keys' actions (`emit_delete_insns`), so a child's old key was found.
- **Bug 4**: `without_rowid::emit_delete` and `emit_update` kept the keys in the order the scan met them (`EphemeralRowidMode::Auto`). A rowid table's DELETE with a foreign key and no trigger kept none: its SET NULL moved the rows it was on (2 of 4 rows left).
- **Bug 3**: the pragma has no name in the parser, so it was an unknown one; `trigger_is_compiling` left the trigger's own call out of its program. Through a foreign key's action the call was there, and nothing at run time refused it (SQLite's OP_Program, P5).
- **A statement refused alone** (no savepoint) left its deferred count: `BEGIN; INSERT` refused, then COMMIT, or a SELECT outside a transaction, refused.

## 4. Fixed there

- `translate/fkeys.rs`: `FkRow`, `emit_fk_row_removed`, `emit_fk_row_added` (`Coming::Alone`, `Among`, `Updated`) are `sqlite3FkCheck`; INSERT, UPDATE, DELETE, UPSERT, REPLACE and the WITHOUT ROWID ones call them where SQLite does. RESTRICT is an action (`fire_fk_restrict`). What they replace is gone (1,073 lines added, 2,158 removed).
- `vdbe/execute.rs`: one count, handed to each program and back; checked where the statement ends, and where FAIL would keep rows.
- `delete.rs`: rows first, in rowid order, for a table with a foreign key (`bComplex`). `without_rowid.rs`: two passes go in the key's order, where SQLite's are two.
- `schema.rs`: a composite parent key's index may list its columns in any order (`sqlite3FkLocateIndex`).
- `PRAGMA recursive_triggers`: a program that fires its own trigger calls itself (`TriggerCompileStack`, by trigger and conflict clause); skipped at run time when the pragma is off; no deeper than 1,000 (sqlite.org/limits.html).
- **A statement's programs run from one flat list** (`run_programs`): each ran the next on the native stack, and a trigger 1,500 deep aborted the process (rc 134, a test thread's 2 MiB) before SQLite's limit. CLAUDE.md 11.

## 5. SQLite 3.54.0 against its own documentation (foreignkeys.html)

The sweep asks `PRAGMA foreign_key_check` after each statement and ends a database where SQLite itself is wrong (17 of the fixture's, 22 of the wide draw's). Amphora follows the documentation in each.

```sql
CREATE TABLE t(id INTEGER PRIMARY KEY, pid INT REFERENCES t(id) ON DELETE SET NULL); REPLACE INTO t VALUES (1, 2);  -- taken; foreign_key_check lists it (isSetNullAction asks of the last program coded)
CREATE TABLE c(a INT REFERENCES p(k), b INT REFERENCES p(k) DEFERRABLE INITIALLY DEFERRED); BEGIN; INSERT INTO c VALUES (1, 2); COMMIT;  -- the INSERT is refused, and then the COMMIT, with nothing broken
```

## 6. Held by

`foreign_keys_and_triggers_leave_what_sqlite_leaves` (`crates/engine/tests/windows_and_triggers.rs`), `fixtures/foreign_keys.tsv` (`bench/sqlite_foreign_keys.py 250 C26`): 526 databases (276 in a finding's own form, 250 drawn), 15,898 questions at default, parallel,
statistics and both, 63,592 asked. It compares what every table and the triggers' log hold, whether the transaction stands, which constraint refused by SQLite's words, and PRAGMA integrity_check.
Red on main a7d4af72 (the first landing's fixture, 432a7e00): 6,552 of 63,848. Green, the first landing's tree: 0. Green, the second's: 0 of 63,592. C22's, C17's and C25's sweeps in the same files: 0.
The second landing's red, its recursive_triggers forms on the tree before it: 14 ON forms, 80 questions (tree 3e657b9e); with the program's own call and the native stack, the process aborted (e2b5e252, rc 134); with the flat list, 106 of 32,952 asked, all REPLACE's
DELETE triggers (section 1).
A wider draw (700 databases, seed wide1, 53,284 asked at default and parallel, tree 23fb21bc, before the deferred count's fix): 12 differ in 3 databases: 82 the deferred count (fixed after), 269 undefined by the documentation, 553 SQLite's own (RELEASE refused with nothing broken).
Not done: each part taken out one at a time (C22's mutants).

## 7. Checks

Linux, adacachy2. First landing (c34b906a on main, 12:45:23Z): `cargo fmt --all --check` 0; `cargo clippy -p amphora-engine --all-targets --locked -- -D warnings` 0 (`Compiling amphora-engine v0.1.0`, r2206); check-production 0 (10,569 left before it; 10,523; the ratchet
lowered: unwrap_used 431 to 421, expect_used 849 to 837, unreachable 457 to 455, indexing_slicing 3036 to 3026, arithmetic_side_effects 3271 to 3261; r2217); windows_and_triggers 5 passed, without_rowid_dml 4, dml_rows 19, upsert_checks 14 (r2200).
Second landing (6683e18d on main, 12:57:35Z, main's cebeb95c and ed7007d4 merged in): fmt 0; clippy 0 (`Checking amphora-engine v0.1.0`); check-production 0 (10,505 left; unwrap_used 419, expect_used 836, indexing_slicing 3012, arithmetic_side_effects 3260; r2247);
windows_and_triggers 5 passed, without_rowid_dml 4, dml_rows 19 (r2253). upsert_checks 14 on 3cd042dd, before cebeb95c (r2241). No assertion of an existing test was changed or taken out.
After the landing, on main 6683e18d (r2260): 700 databases, seed wide2, 53,480 asked at default and parallel: 6 answers differ, in one database, 697: `UPDATE OR FAIL p SET n = k WHERE +m < 'c'` meets its conflict on another row first, as it goes by the index
Amphora picks and SQLite by rowid (no trigger, foreign key or REPLACE; section 1's last kind). SQLite broke its documentation in 38 of that draw's.

## 8. Reproduce

```sh
python3 bench/sqlite_foreign_keys.py 250 C26 > crates/engine/tests/fixtures/foreign_keys.tsv   # as committed; 700 wide2: wider; C26_REPLACED=1: REPLACE under recursive_triggers too
cargo test -p amphora-engine --locked --test windows_and_triggers
```

Lost time: the first six minutes, adacachy2 with no idle core and the Macs' queue an hour long. Scratch refs on GitHub, never merged: perf/dml-c26-wip, -red, -recursive, diag/dml-c26 (the probe and the driver).
