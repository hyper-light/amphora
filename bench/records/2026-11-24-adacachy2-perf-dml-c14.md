# Lane C14 (dml), 2026-11-24 09:41:37Z to 10:41:37Z: excluded.<INTEGER PRIMARY KEY> (the oracles' finding DA), what a BEFORE trigger sees on a STRICT table (CV), and their class held to SQLite's stored rows

Machine: adacachy2 (runner 00aad8da, Linux, 32 cores, 1-minute load 27 to 209 from other lanes). Every build debug (`simulator` feature), `nice -n 10`, under timeouts.
Started on main 70f6712b. Reference: SQLite 3.54.0 (Python's, adacachy; the sweep's generator and the tests' answers). Nothing was timed.

## 1. Losses first

- **SQLite's `excluded.c` is not always the value as its column converts it, and Amphora's is.** At a conflict on the rowid SQLite reads the value as given:
  `t(id INTEGER PRIMARY KEY, c INT, d BLOB)`, `VALUES (1, '7', NULL) ON CONFLICT(id) DO UPDATE SET d = excluded.c` stores `'7'`; Amphora stores 7, and both store 7
  at a conflict on an index, if the table has a generated column, or if an earlier ON CONFLICT names another constraint (each run on SQLite). On a STRICT table the same statement with `'x'` runs on SQLite and is refused by Amphora (its type is checked first).
  lang_upsert.html (read 10:34Z) says "the value that would have been inserted had the constraint not failed", which is Amphora's. Left as it is, left out of the sweep (below), a decision for Ada.
- **Left on main, found on the way.** An UPDATE of the rowid beside a generated column that reads it (`g INT AS (id)`, `UPDATE t SET id = 3`): an AFTER trigger's NEW.g is 1, SQLite's 3 (the row, g's index and integrity_check are right; r1798).
  A subquery in DO UPDATE (`SET c = (SELECT excluded.c + 1)`) is refused ("Subquery is not supported in this position"). STRICT beside WITHOUT ROWID and triggers on WITHOUT ROWID
  tables are refused (C11's list), so the sweep holds neither. INSERT OR IGNORE of a NULL into a STRICT key that is no rowid alias is refused (C11's list).
- **Generated columns need the engine's flag** (`with_generated_columns`), which `crates/amphora` does not set, so the wrong answers of a generated column are not reachable in production today.
- **Not run:** versioned storage and MVCC, a release build, the Postgres dialect (a grep of crates/engine/dialect, crates/pg-analysis and crates/amphora/src finds no STRICT table; nothing was run on it), the corpus, the engine's whole unit suite (176 of its tests, section 6).
- **The sweep is no oracle.** Its shapes are the generator's: no CHECK, no foreign keys, no partial or expression index, no RETURNING, no custom type; collation only in DO UPDATE's comparisons.
- **One claim of a commit message is not shown.** 5572c3ed says g's index entry was wrong too; the test stops at the trigger's row on main, so no run shows it. And the fix commits were not each run on main's tip: each was swept as first made (section 5), the tree as landed.
- **Lost time:** 5 runs refused for room (5 to 7 of 32 cores idle), one on a push that failed for want of a git identity, check-production red once (two `then_some`, r1717; gone), and three merges of main, each re-run.

## 2. Reproduced

Amphora on main 70f6712b, each in default, parallel and statistics (the same in all three), read back by SELECT; SQLite 3.54.0 beside it.

| finding | statements | SQLite | Amphora |
|---|---|---|---|
| DA, SET | `item(id INTEGER PRIMARY KEY, qty INT, last INT) STRICT`, (10, 1, 0); `VALUES (10, 5, 0) ON CONFLICT(id) DO UPDATE SET qty = item.qty + excluded.qty, last = excluded.id` | 10, 6, 10 | 10, 6, NULL |
| DA, WHERE | `.. DO UPDATE SET qty = excluded.qty WHERE excluded.id = 10` | 10, 5 | 10, 1 |
| DA, SET id | `.. DO UPDATE SET id = excluded.id + 1` | 11, 1 | refused: datatype mismatch |
| DA, not STRICT too | `kv(id INTEGER PRIMARY KEY, name TEXT UNIQUE, n INT)`, (1, 'a', 0); `kv(name, n) VALUES ('a', 5) ON CONFLICT(name) DO UPDATE SET n = excluded.id` | 1, a, 2 | 1, a, NULL |
| CV, INSERT | `price(id INTEGER PRIMARY KEY, amount REAL) STRICT`, BEFORE INSERT logs new.amount; `VALUES (1, 100)` | 100.0, real | 100, integer |
| CV, UPDATE | `n(id INTEGER PRIMARY KEY, v INTEGER) STRICT`, BEFORE UPDATE logs new.v; `SET v = '7'` | 7, integer | '7', text |

Run on a table that is not STRICT: DA's first answers as SQLite does; the fourth is wrong with it or without.

## 3. Traced

| what | bytecode and row, main | fixed |
|---|---|---|
| DA | `24 Copy 3 16 2` copies the row to insert to r16..r18; r3, the alias's, is `3 SoftNull 3` (the record stores NULL; the key is r2). `25 Copy 16 12` reads r16 for excluded.id: r12 (last) is NULL. Not STRICT: `22 Copy 2 12` reads the key | `25 Copy 2 12`: r12 is r2, 10 |
| CV, INSERT | `4 Integer 100 4`, `9 Program 4` (the trigger), `16 TypeCheck 3 2` after it | `9 TypeCheck 3 2`, `11 Program 4` |
| CV, UPDATE | `15 Program 6`, `21 TypeCheck 5 2` after it | `14 TypeCheck 5 2`, `16 Program 6` |

- **DA's first wrong place:** `rewrite_expr_to_registers` (translate/upsert.rs) took `excluded.<column>` of a STRICT table from the decoded copy, whatever `Insertion::get_col_mapping_by_name` had answered, and that
  function knows an alias only where the INSERT lists it. `excluded.<alias>` is now the key register, whichever way the key comes.
- **CV's:** a STRICT table's TypeCheck is what converts, and it stood after the BEFORE triggers in `translate_insert` and `emit_update_insns` (the affinities of a table that is not STRICT stand before).
  It now stands before them (and before the generated columns NEW holds), as SQLite's `sqlite3TableAffinity` does before `sqlite3CodeRowTrigger`; the rowid of an INSERT is made an integer there as well, on any table. A NULL in a
  primary key is still refused after the triggers (`check_null_keys`), as SQLite's NOT NULL is: a trigger that RAISE(IGNORE)s a NULL key still ignores it.

## 4. What the sweep found beside them, and the fix

The sweep compared the fixed tree, and four more wrong answers stood in the same functions. Each writes or shows a wrong value, so each is fixed, its test red on main.

| what | SQLite | Amphora on main | fix |
|---|---|---|---|
| DO UPDATE sets a REAL to 6 on a table with `g TEXT AS (c || 'z')`: NEW.g of an AFTER trigger | `6.0z` | `6z` | the affinities go before the generated columns (`emit_upsert`) |
| a STRICT table's excluded value compared: `excluded.c = 'abc'` (NOCASE, 'ABC'), `excluded.n = '5'` | 0, 0 | 1, 1 (not STRICT: 0, 0) | excluded registers carry no affinity and no collation on any table |
| a column declared `oid` or `rowid`: excluded.oid, oid, t.oid in DO UPDATE | the column | the key | the declared column first |
| a column declared `oid TEXT` in a buffered INSERT (several rows, a trigger): 1.0 | '1.0' | '1' | the column's own affinity (`init_source_emission`) |
| a name that is no column: `DO UPDATE SET c = nosuch`; `excluded.rowid` WITHOUT ROWID | refused | the text `nosuch` stored; a register read | refused; only a double-quoted name is a string |

## 5. Held

`crates/engine/tests/dml_rows.rs`: eight hand-written tests and the sweep in eight parts, 16 in all. `bench/sqlite_dml_rows.py COUNT SEED` makes a case: a table (an alias of the rowid as a column's PRIMARY KEY, a table's or AUTOINCREMENT, listed
in the INSERT or not; `INTEGER PRIMARY KEY DESC`, INT or TEXT, which are none; no key; WITHOUT ROWID; STRICT with INT, INTEGER, REAL, TEXT, BLOB and ANY; the affinities otherwise; defaults, NOT NULL, collations; a generated column, its index; a column named as the rowid), up to four
triggers of BEFORE and AFTER INSERT, UPDATE, UPDATE OF and DELETE logging every NEW and OLD value with its type, and INSERT, UPSERT (DO UPDATE, DO NOTHING, WHERE, several rows, SELECT, OR clauses), REPLACE and UPDATE statements with
21 shapes of `excluded.*`. Each is compared statement by statement (done or refused) and by the rows of the table, the log and `integrity_check`, under default, parallel and statistics.
A case is left out where SQLite's answer depends on whether it reads excluded unconverted at a conflict on the rowid (the same case on a table with one more generated column; 11 to 18 of 1,500).

| tree | cases | runs | wrong |
|---|---:|---:|---:|
| main bc008095 (red, r1775): the fixture (420 1) | 420 | 1,260 | 231; 16 of 16 tests fail |
| main bc008095 (r1775): 1500 1 | 1,500 | 4,500 | 855 |
| DA's fix alone (cf501d8c; an earlier generator) 1500 1 | 1,500 | 4,500 | 663, none in the table's rows |
| DA and CV (5a80888f) 1500 1, 1500 2 | 3,000 | 9,000 | 15 (generated columns; excluded's affinity) |
| and the generated columns and affinities (2b9b8a66), columns named as the rowid in the generator, 3 seeds | 4,500 | 13,500 | 54, all on columns named as the rowid |
| the whole fix (91094db3): seeds 1, 3 (1,500 each), 5 (3,000) | 6,000 | 18,000 | 0 |
| merged (53cd4ada): seeds 1, 2 | 3,000 | 9,000 | 0 |

No panic in any run.

## 6. Checks

All on adacachy2.

- The land tree on main 5fa71186 (3434d54e, r1786): fmt 0, clippy -p amphora-engine --all-targets 0 ("Checking amphora-engine"), check-production 0 (10,616, as the ratchet holds), dml_rows 16, upsert_conflict_clauses 1,
  upsert_or_clause 2, oracle_findings 91, without_rowid_dml 4, without_rowid_affinity 2, row_changes 4, outer_join_subqueries 2, plan_cache 7, all passed. Main moved under it twice (lanes C16 and C12, in crates/engine):
  the tree on 4b5c78b4 (f519e7ed, r1798): clippy 0 (checked), check-production 0 (10,606), dml_rows 16, upsert_conflict_clauses 1, upsert_or_clause 2, oracle_findings 91, without_rowid_dml 4, without_rowid_affinity 2, row_changes 4.
  then lanes C12 and PG22: the tree on 58cd91cd (affa8493, r1810): clippy 0 (checked), check-production 0 (10,600), dml_rows 16, upsert_conflict_clauses 1, upsert_or_clause 2, oracle_findings 92, without_rowid_dml 4;
  **landed as ac3cf51f, 10:33:04Z** (on b5584c1e, r1817, the tree that was pushed): fmt 0, clippy 0 (checked), check-production 0 (10,600, as the ratchet holds), dml_rows 16, upsert_conflict_clauses 1, upsert_or_clause 2, oracle_findings 92, without_rowid_dml 4.
- check-production was red once (r1717: `bool::then_some` twice, banned); the fix is in the tree above.
- Triggers' own bodies (r1717, `trig.sql`): an UPSERT in a trigger with excluded.<alias>, an INSERT alias, RETURNING after DO UPDATE, a table named excluded, INSERT SELECT from the table itself: 5 forms under three settings, each as SQLite.
- After landing, on ac3cf51f (r1822): the engine's unit tests whose names hold upsert, trigger, strict, insert, replace, generated or custom_type: 176 passed, 0 failed, 10 ignored (3,095 filtered out).

## 7. Reproduce

```sh
python3 bench/sqlite_dml_rows.py 420 1 > crates/engine/tests/fixtures/dml_rows.tsv          # SQLite 3.54.0 in Python
cargo test -p amphora-engine --locked --features simulator --test dml_rows
python3 bench/sqlite_dml_rows.py 1500 2 > wide.tsv    # a wider run: the same test reading wide.tsv (diag/dml-c14's probe/c14_dml.rs, not for main)
```

## 8. Decision for Ada

At a conflict on the rowid, SQLite gives DO UPDATE the value as typed in the statement, not as its column would store it (`'7'` in an INT column stays the text `'7'`). It does so nowhere else, and its own page says otherwise.
Amphora gives the converted value everywhere. Matching SQLite means the same statement stores a different type in the same table by which constraint it met; keeping Amphora's means 11 to 18 of 1,500 generated cases store another row than SQLite
(a value of another type than its column's, put in a column of another type or asked its type).
