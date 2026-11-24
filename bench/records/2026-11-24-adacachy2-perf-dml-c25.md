# Lane C25 (dml), 2026-11-24 11:34:28Z to 12:34:28Z: a window's columns under `*`, an UPSERT's checks beside its triggers, the pragmas' table functions after a refused statement

Machine: adacachy2 (runner 00aad8da, Linux, 32 cores, 1-minute load 13 to 49 from other lanes). Every build debug (`simulator` feature), `nice -n 10`, under timeouts. Nothing was timed.
Started on main 2ba5f5bb. Reference: SQLite 3.54.0 (Python's, adacachy and adacachy2). "Red" below is main's engine beneath this lane's tests.

## 1. Losses first

- **Bug 4 (an UPSERT beside a BEFORE UPDATE trigger and an index on a generated column, "Corrupt database") is left:** nothing but the engine's own tests and macros calls `with_generated_columns`, so production doesn't reach it. An index on an expression is reachable and is in the sweep: right.
  With the flag on, two shapes of it (the trigger changes a column the generated one reads; it only logs) end right on this tree (r2115); C19's own statement is in no record, so main was not asked.
- **Found and left, each a cause of its own (Amphora's answers are this tree's: r2115, and fail.sql on 5dd57503 at 12:16Z):**
  - **Wrong data:** a trigger's RAISE(FAIL) while the statement has broken an immediate foreign key keeps the row with no parent. `t(id INTEGER PRIMARY KEY, f REFERENCES p(x))`, rows (1, 1), (2, 1), BEFORE UPDATE .. WHEN old.id = 2 .. RAISE(FAIL, 'no'): `UPDATE t SET f = 8` leaves (1, 8). SQLite: FOREIGN KEY
    constraint failed, nothing changed (sqlite3VdbeCheckFk). OR FAIL of a constraint is right.
  - **A statement SQLite takes is refused:** an immediate foreign key is judged at the row, not at the statement's end (foreignkeys.html: "at the conclusion of the statement"). `emp(id INTEGER PRIMARY KEY, boss REFERENCES emp(id))`: `INSERT INTO emp VALUES (2, 1), (1, NULL)` is FOREIGN KEY constraint failed. Also an UPSERT whose second row mends what its first broke.
  - **The same, rarer:** `INSERT OR REPLACE .. ON CONFLICT(id) DO UPDATE` that replaces the row a child's parent key is in (the new row has the key): FOREIGN KEY constraint failed, on a WITHOUT ROWID table (r2080, case 29 of a draw since replaced) and on a rowid table (r2129, seed 4, case 26); a plain INSERT OR REPLACE of one row is right. Not reduced further.
  - `pragma_index_list` of a WITHOUT ROWID table leaves out its primary key's index; `pragma_integrity_check('t')` is refused ("expected at most 0"); `pragma_foreign_key_check` is no table; `pragma_integrity_check`'s column is called `message` (SQLite `integrity_check`).
  - `integrity_check` names a row once for each CHECK it breaks, SQLite once.
  - `CREATE TABLE t(id TEXT PRIMARY KEY, CHECK (id <> 9)) STRICT`: refused ("type mismatch in CHECK constraint"), SQLite takes it.
  - A column's name where no AS names it (c3ref/column_name.html: "unspecified"): `SELECT t.a, s.a, sum(t.a) OVER () ..` names the second `a:1` (SQLite `a`); a correlated scalar subquery unnested to a join is named by the subquery's own column (`max(b)`; with statistics, by its text); `(a)` by its text; a view's
    by its text as rewritten (`avg (g) OVER ..`); `"rowid"` over a subquery's rowid column is a string. Not compared by the sweep.
  - SQLite 3.54.0's own: one case of 1,500 (seed 3) ends with its integrity_check "Rowid 3 out of order" (a BEFORE trigger that changes the row it fires for, undefined by lang_createtrigger.html). Such cases are no oracle and are dropped.
- Which of two UNIQUE constraints a row meets first is not held (no document says); the refusal is held as UNIQUE's.
- Not run: versioned storage and MVCC, a release build, the Postgres dialect, the corpus, macOS, the engine's whole unit suite.
- **The UPSERT generator's last exclusion is too narrow:** it leaves OR REPLACE beside foreign keys out on WITHOUT ROWID tables alone; seed 4 met the same cause on a rowid table (above). The fixture (seed 1) doesn't.
- **Lost time:** three runs the kiln server dropped ("did not answer in time") and two refused for room, about six minutes; the trace commit made before the clone had a git identity; a hand test whose own INSERT row broke the CHECK; the fixture redrawn five times as causes outside the class came up.

## 2. Shown on main, smallest (r2027, main 2ba5f5bb, the same in default, parallel, statistics and both)

| | statement | SQLite | main | now |
|---|---|---|---|---|
| 1 | `SELECT * FROM (SELECT a, avg(g) OVER (ORDER BY a), min(a) OVER (ORDER BY a DESC) FROM t)` | 3 columns | 2 | 3 |
| 1 | the same beside `INSERT INTO d(a, b, c) SELECT * ..` | 3 rows in | table d has 3 columns but 2 values were supplied | 3 rows in |
| 1 | one window is enough: `SELECT * FROM (SELECT g, count(*), sum(count(*)) OVER () FROM t GROUP BY g)`; a scalar subquery, EXISTS or IN beside `sum(a) OVER ()` | 3 columns | 2 | 3 |
| 2 | `t(id INTEGER PRIMARY KEY, n INT NOT NULL, c INT CHECK (c > 0))`, BEFORE UPDATE trigger that RAISEs 'before': `.. ON CONFLICT(id) DO UPDATE SET n = NULL` | before | NOT NULL constraint failed: t.n | before |
| 2 | the trigger RAISEs IGNORE, or deletes the row | done, the log has NEW | refused | done |
| 2 | `CHECK (a < b)`, row (1, 1, 10), a BEFORE UPDATE OF a trigger sets b = 13 - b: `.. VALUES (1, 1, 2) ON CONFLICT(id) DO UPDATE SET a = 5` (r2129, main 8be208d6) | CHECK constraint failed: a < b | **done: 5 and 3 stored, a row that breaks its CHECK** | refused |
| 2 | `.. DO UPDATE SET b = NULL, a = NULL`, both NOT NULL | t.a | t.b | t.a |
| 3 | `SELECT * FROM pragma_integrity_check` after a statement a constraint ended | ok | Internal error: No row available | ok |

## 3. Traced to the first wrong step

| what | where | fixed |
|---|---|---|
| 1 | `plan_windows` hands each window's layer its input as a SELECT beneath. A result column that is a call of another window, an aggregate or holds a subquery is moved down whole, and is now a reference to the layer's column, which has no name; `ResultSetColumn::name` names a reference by the column it reads, so the name is lost, and `expand_star` passes over a column without one | the column keeps its own text as its name where it becomes such a reference (`prepare_window_subquery`), as SQLite names columns (sqlite3ColumnsFromExprList) before it rewrites windows |
| 2 | `emit_upsert` (rowid tables) checked NOT NULL as it evaluated each SET and CHECK before it fired the BEFORE UPDATE triggers, whose rows it then re-read without checking them | sqlite3Update's order: the types, the triggers, the re-read, then NOT NULL (the table's first column first) and CHECK; NOT NULL before the types where there are no triggers |
| 2 | the same path checked every CHECK, and WITHOUT ROWID's UPDATE and UPSERT too (`validate_image`): a row stored under `ignore_check_constraints` couldn't be updated elsewhere | the CHECKs that read a column the UPDATE sets, by one function for UPDATE and UPSERT, rowid and WITHOUT ROWID (`check_constraints_of_update`) |
| 2 | foreign keys (RESTRICT, NO ACTION) were judged before UNIQUE (seen on 609a3a59, r2048, whose order is main's) | after it, as sqlite3FkCheck follows sqlite3GenerateConstraintChecks |
| 2 | `create_table` collected a table's CHECKs before its columns' | as they are written |
| 3 | the pragma's cursor took every step of its statement but Done for a row; after a refused statement the first step is a Yield (traced: "pragma integrity_check step Yield", r2038) | only a row is a row; what the statement waits on is handed to the caller (`IOResult::IO`), and a filter called again goes on with the statement it prepared |

## 4. Held

`bench/sqlite_window_columns.py COUNT SEED`: an inner SELECT of up to three windows (inline, named; calls alone, twice, inside expressions) beside columns, the rowid, aggregates, scalar subqueries, EXISTS and IN, named or not; over a table, a join, groups, WITHOUT ROWID, DISTINCT, ORDER BY and LIMIT; read through `*` and `q.*` in FROM, a CTE
(MATERIALIZED too), a view, either arm of a compound, joins, another window, a subquery in WHERE, by a column's name, CREATE TABLE AS and INSERT. `fixtures/window_columns.tsv`: 1,200 cases.
`bench/sqlite_upsert_checks.py COUNT SEED`: tables with a rowid, an alias or WITHOUT ROWID, STRICT or not, whose columns are NOT NULL, CHECKed, UNIQUE, a child and a parent of foreign keys, each with a conflict clause or none; an index, plain, partial, unique or on an expression; triggers BEFORE and AFTER each event that log, RAISE or write the row they
fire for; UPSERTs of 1 to 3 rows under every OR clause, with one ON CONFLICT or two, RETURNING, beside UPDATE, INSERT and DELETE, alone or in a transaction; a pragma's table function after each refusal. The refusal's text, the rows, the triggers' log and `integrity_check` are held. `fixtures/upsert_checks.tsv`: 600 cases.
Hand-written: `tests/window_columns.rs` (1, with the columns' names), `tests/upsert_checks.rs` (6). Every one is red on main.

| tree | what | runs (4 settings) | wrong |
|---|---|---:|---:|
| main 8be208d6 (red, the tests' commit alone: ab798059, r2115) | windows, 1,200 cases | 4,800 | 1,460; 9 of 9 tests fail |
| the same (16a75d47, r2129, the fixture as landed) | UPSERTs, 600 cases | 2,400 | 1,422; 14 of 14 tests fail |
| the landing (430e5929, r2129) | windows, 1,200 cases | 4,800 | 0; 9 of 9 pass |
| the same | UPSERTs, 600 cases | 2,400 | 0; 14 of 14 pass |
| the same engine (bf6ae3fa r2087, 5dd57503 r2121, 430e5929 r2129) | windows 2500 2, 2500 3, 2500 4 | 30,000 | 0 |
| the same (bf6ae3fa r2087, 430e5929 r2129) | UPSERTs 1500 2 (before transactions were drawn), 1500 3 | 12,000 | 0 |
| the same (430e5929, r2129) | UPSERTs 1500 4 | 6,000 | 4: one case, OR REPLACE beside a foreign key (section 1) |

Beside the sweep, 5 cases of refused UPSERTs inside BEGIN .. COMMIT whose BEFORE trigger had written a log (rowid, WITHOUT ROWID, STRICT, OR IGNORE, OR REPLACE, OR FAIL): the log and the rows are SQLite's (r2121). 30% of the UPSERT cases are drawn in a transaction.

The sweeps found, beside the three: foreign keys before UNIQUE, the CHECKs' order, the CHECKs of columns not set. Lane C22's landing (3a8a80a0, the conflict action is the instruction's) took away what else they met: an UPSERT under OR FAIL and OR ROLLBACK, the text of a WITHOUT ROWID refusal.

## 5. Checks

All on adacachy2.

- f8a2988a (main 24992ce6 merged, r2096; its engine is the landing's, by diff, but for the ratchet file): windows_and_triggers 4, dml_rows 19, refused_queries 6, cte_scopes 1 (4,800 answers), oracle_findings 102, upsert_conflict_clauses 1, upsert_or_clause 2, without_rowid_dml 4, row_changes 4, cte_columns 11, columns_read 5, row_values 4, all passed; clippy 0.
- 42ec4bd7 (r2115, the same engine): check-production 0 (10,567, as the ratchet holds; unwrap_used 432 to 431), clippy -p amphora-engine -p amphora-engine-parser --all-targets 0 ("Checking amphora-engine v0.1.0").
- The tree that was pushed, 430e5929 (main 988f0fb4, r2129): fmt 0, clippy 0 ("Checking amphora-engine v0.1.0"), upsert_checks 14, window_columns 9, oracle_findings 102, dml_rows 19. **Landed as 430e5929, 12:19:04Z.**
- 430e5929, the library's tests that touch it (r2146; pragma, schema::, upsert, stmt_journal, window, check_constraint, vtab, translate::): 408 passed, 0 failed, 22 ignored; run after the push.

Not run on the pushed tree itself: check-production, windows_and_triggers, refused_queries, cte_scopes (the engine is byte for byte the one they passed on), the rest of the engine's unit suite.

## 6. Reproduce

```sh
python3 bench/sqlite_window_columns.py 1200 1 > crates/engine/tests/fixtures/window_columns.tsv      # SQLite 3.54.0 in Python
python3 bench/sqlite_upsert_checks.py 600 1 > crates/engine/tests/fixtures/upsert_checks.tsv
cargo test -p amphora-engine --locked --features simulator --test window_columns --test upsert_checks
# wider: another COUNT and SEED in the fixture's place
```
