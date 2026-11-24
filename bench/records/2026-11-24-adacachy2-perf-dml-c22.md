# Lane C22 (dml), 2026-11-24 11:11:16Z to 12:11:16Z: a conflict is judged by the clause of the statement that meets it, in a trigger's program too; C17's unrestricted draw explained

Machine: adacachy2 (runner 00aad8da, Linux, 32 cores, load 17 to 35). Every build debug, 8 cores, `nice -n 10`, under `timeout`. SQLite 3.54.0 (Python's sqlite3 module, adacachy's system library).
Goal G2 step 3; AC-T1.1.1, AC-T1.8.6; CLAUDE.md 1, 4. Nothing was timed.

## 1. Losses first

- **One assertion of an existing test is changed.** `upsert_update_aborts_without_changing_outer_fail_policy` (tests/without_rowid_dml.rs) asserted a DO UPDATE's three refusals as `LimboError::Raise(Abort, _)`,
  the variant that carried the action when the action was the program's. They are now `LimboError::Constraint` with SQLite's words, as a rowid table's are and as SQLite's result codes say
  (rescode.html: SQLITE_CONSTRAINT_NOTNULL, _UNIQUE, _CHECK; asked of 3.54.0). What it says of the rows is untouched, and the new assertion holds the words too.
- **Not fixed, each of another cause** (SQLite's answer, then Amphora's on this tree):
  - `PRAGMA recursive_triggers = ON` is taken in silence and has no effect: `CREATE TABLE t(k INTEGER PRIMARY KEY, n INT); CREATE TRIGGER g AFTER INSERT ON t WHEN new.n < 4 BEGIN INSERT INTO t(n) VALUES (new.n + 1); END;
    PRAGMA recursive_triggers = ON; INSERT INTO t VALUES (100, 1)`: 4 rows; 2. A REPLACE then fires no DELETE trigger either. So the clause of a trigger REPLACE fires (SQLite: REPLACE) can't be asked.
  - **A trigger's program refuses a foreign key's violation where it ends**, SQLite where the statement does: `CREATE TABLE u(k TEXT PRIMARY KEY); CREATE TABLE w(k TEXT REFERENCES u(k)); CREATE TABLE t(a);
    CREATE TRIGGER h AFTER INSERT ON t BEGIN INSERT OR IGNORE INTO u VALUES ('q'); END; CREATE TRIGGER g AFTER INSERT ON t BEGIN INSERT INTO w VALUES ('q'); END; INSERT INTO t VALUES (1)` under foreign_keys:
    taken; FOREIGN KEY constraint failed (`halt`, the check before `is_trigger_subprogram`).
  - **ON DELETE SET DEFAULT whose default is the key deleted leaves an orphan** (wide3's 110, 148, 505, 540, 863; read in section 5).
  - **A DELETE (or UPDATE) of several rows in two passes goes in the scan's order; SQLite's goes in key order** (wide1's 332 and 399, which C17 put down to the planner's choice of index: both engines scan the same index there,
    EXPLAIN QUERY PLAN on each). SQLite's EXPLAIN puts the rows' keys in an ephemeral index keyed by the primary key and deletes in its order (OpenEphemeral, IdxInsert, Rewind), so triggers that write the table meet the rows in it. 343 is the
    planner's: SQLite scans t, Amphora its covering index tca, and an OR IGNORE keeps the first row met.
  - Words: STRICT's "cannot store TEXT value in INT column t.a (19)" (SQLite without the code), "Parse error: malformed JSON", "Transaction error: cannot commit - no transaction is active".
- **Cost, not measured.** A statement of several rows that gives a rowid under IGNORE, FAIL, REPLACE or ROLLBACK, inside a transaction, opens a statement savepoint it didn't (its MustBeInt may abort, as SQLite's does: the rows
  it wrote go). A plain INSERT has always opened one.
- Main moved twice while this lane worked. C19 (df4fc9da) took the swallow out of `op_program` first, which is what the brief's own example met; red is given on fcad33f7 and on main with it.

## 2. Reproduced (SQLite 3.54.0's rows, errors and log; default and parallel the same at each)

The sweep below on fcad33f7, 31,286 asked at default and parallel (its first form, 300 drawn): 1,402 answers differ and 100 more in which constraint refused. On main 2ba5f5bb, as committed, each of four settings (14,612 questions): 617 differ (616 with statistics).

| part | databases that differ | questions | what differs |
|---|---:|---:|---|
| ladder: a clause on the statement by a clause on its trigger's (INSERT, UPDATE, DO UPDATE; firing INSERT, UPDATE, DO UPDATE; rowid and WITHOUT ROWID) | 58 of 108 | 388 | FAIL and ROLLBACK in the trigger beside another statement; ROLLBACK outside over the trigger's |
| nested: a clause on each of three levels (6 by 6 by 6) | 8 of 36 | 20 | the same, two triggers deep |
| a DO UPDATE under FAIL or ROLLBACK | 1 of 1 | 7 | rows FAIL kept, a transaction ROLLBACK ended: SQLite undoes the statement |
| a trigger's statement beside the next one's | 1 of 1 | 12 | the last statement's clause said it for all |
| a DELETE in a trigger | 1 of 1 | 3 | an outer IGNORE or FAIL reached what it fires |
| foreign keys' actions | 2 of 2 | 6 | the child's ON CONFLICT REPLACE, its trigger's OR IGNORE: SQLite's is ABORT |
| NOT NULL ON CONFLICT | 1 of 1 | 8 | FAIL and ROLLBACK were ABORT; after a BEFORE UPDATE trigger the statement's |
| a rowid that is no integer | 1 of 1 | 7 | ROLLBACK ended the transaction, FAIL kept the rows before it |
| UNIQUE ON CONFLICT IGNORE beside an upsert | 1 of 1 | 3 | refused |
| a WITHOUT ROWID table's key | 12 of 108 | 60 | the key's clause judged a row another constraint is met on first |
| drawn (250 databases) | 39 | 103 | each of the above |

## 3. Traced to the first place

`PreparedProgram.resolve_type`: one clause a program, set by each INSERT and UPDATE as it's translated (`set_resolve_type`), read when a constraint's error is undone (`abort`, `fail_step`, `halt`). A trigger's program holds several statements, so
the last one's clause was every one's; an error that left the program was judged by the clause of the statement that ran it; a DO UPDATE's and a datatype mismatch's by their INSERT's. SQLite's is compiled into each OP_Halt (P2) and is the error's from
there (`Vdbe.errorAction`, ABORT until a Halt says otherwise). Fixed there: `HaltAction` on `Halt` and `HaltIfNull`, from the clause the constraint is checked under; `ProgramState::constraint_action`, which `op_program` hands to the statement that
ran the program. `resolve_type` is gone from the prepared program.

The rest of the class, each a constraint judged by a clause not its own: `emit_deferred_notnull_checks` (update.rs: the statement's, not the column's), the upsert path of `emit_index_uniqueness_check` (insert.rs: no IGNORE), the key's place in
`build_constraints_to_check` and `without_rowid::check_constraints` (always first; SQLite lists it with the indexes, newest first, REPLACE last), the foreign keys' generated UPDATEs (`or_conflict: None`; SQLite's `sqlite3FkActions`
codes them with OE_Abort), `ProgramBuilder::may_abort` (a MustBeInt under FAIL, IGNORE or ROLLBACK had no statement savepoint).

## 4. C17's unrestricted draw (900 databases, seed wide1; 15,222 questions, default and parallel)

| tree | questions that differ | databases |
|---|---:|---|
| fcad33f7 | 44 | 17: 13, 23, 129, 144, 215, 236, 260, 319, 332, 343, 392, 399, 465, 489, 523, 586, 651 |
| this lane's, before main's C21 | 11 | 144 (NOCASE in a WHEN, the oracles' CX: C21 fixed it), 332, 343, 399 |
| as landed | 10 | 332, 343, 399 |

Each explained, by the part of the fix it comes back without (section 5) and its first differing question read: 13 a datatype mismatch under ROLLBACK (the transaction's rows lost); 215, 319, 465, 489, 523 the clause of a trigger's
statement (523 is the brief's example, which C19's df4fc9da fixed first); 260 a DO UPDATE under FAIL (the BEFORE trigger's log kept); 129, 392 UNIQUE ON CONFLICT IGNORE beside an upsert; 23, 236, 586 the WITHOUT ROWID key's place (236 lost a row); 651 NOT NULL ON
CONFLICT REPLACE after a BEFORE UPDATE trigger; 144 NOCASE in a WHEN (C21's fix); 332, 399 the order a two-pass DELETE goes in, 343 the index
an UPDATE scans (section 1; no part of this fix moves them).

## 5. Held by

`a_conflict_is_judged_by_the_clause_of_the_statement_that_meets_it` (`crates/engine/tests/windows_and_triggers.rs`), `fixtures/trigger_conflicts.tsv` (`bench/sqlite_trigger_conflicts.py 250 C22`): 512 databases (10 in a finding's own
form, 108 of the WITHOUT ROWID key's place, 36 nested three deep, 108 of the ladder, 250 drawn), 14,612 questions at default, parallel, statistics and both, 58,448 asked. It compares what every table and
the triggers' log hold, whether the transaction stands (the COMMIT or ROLLBACK after), PRAGMA integrity_check, and which constraint refused, by SQLite's words (as a plain refusal for a drawn statement beside foreign keys).
Red on main 2ba5f5bb 2,466 of 58,448; green 0. C17's triggers sweep 0 of 17,596.

Each part taken out of the landed tree (3a8a80a0), asked at default and parallel (a scratch probe, not committed; 29,224 asked of the sweep, 30,444 of C17's unrestricted draw):

| part taken out | the sweep: answers that differ (and in which constraint alone) | C17's draw: databases that differ beside 332, 343, 399 |
|---|---:|---|
| main 2ba5f5bb, nothing of this lane | 1,180 (54) | 13, 23, 129, 215, 236, 260, 319, 392, 465, 489, 586, 651 |
| a Halt's action (FAIL and ROLLBACK as ABORT) | 1,576 (2) | 185 (215, 319, 465, 489 and 523 among them) |
| the action handed to the statement that ran the program | 1,106 (2) | 58, with 215, 319, 465, 489 and 523 among them |
| the key's place | 520 (40; 128 panics of the REPLACE-last assertion) | 23, 236, 586 |
| NOT NULL's own clause in an UPDATE | 10 (2) | 130, 477, 593, 647, 651 |
| MustBeInt may abort | 14 (2) | none (its form is the sweep's) |
| IGNORE beside an upsert | 6 (0) | 129, 392 |
| ABORT on a foreign key's action | 18 (0) | none (drawn 87 of the sweep, and the two forms) |

A wider draw of the sweep (900 databases, seed wide3, 56,040 asked at default and parallel, tree 69df526d): 25 questions differ, in six databases, all of foreign keys: 153 the violation refused at a trigger's end (section 1); 110, 148, 505, 540, 863
ON DELETE SET DEFAULT with the key that is deleted for its default (each differs at the same question on fcad33f7):

```sql
PRAGMA foreign_keys = ON; CREATE TABLE u(k TEXT PRIMARY KEY); CREATE TABLE w(k TEXT DEFAULT 'b' REFERENCES u(k) ON DELETE SET DEFAULT); INSERT INTO u VALUES ('b'); INSERT INTO w VALUES ('b');
DELETE FROM u WHERE k = 'b';   -- SQLite: FOREIGN KEY constraint failed. Amphora (110, 505 and 863): taken, w holds an orphan
-- and REPLACE INTO u VALUES ('b', 9) where u has the key under REPLACE: SQLite takes it, Amphora (148, 540): FOREIGN KEY constraint failed
```


## 6. Checks

Linux, adacachy2, the tree landed (3a8a80a0; r2067): `cargo fmt --all --check` 0; `cargo clippy -p amphora-engine --all-targets --locked -- -D warnings` 0 (`Checking amphora-engine v0.1.0`); check-production 0 (10,569 left;
indexing_slicing 3038 to 3036, arithmetic_side_effects 3273 to 3271); windows_and_triggers 4 passed (82,996 answers), without_rowid_dml 4, without_rowid_seek 6, upsert_conflict_clauses 1, upsert_or_clause 2, dml_rows 19. On the same change before
main's decision 45 was merged in (69df526d; r2028, r2040): without_rowid_affinity 2, oracle_findings 100, row_values 4, refused_queries 6, cte_scopes 1; the library's `vdbe::` 403 (4 ignored), `translate::` 298 (22 ignored), `trigger` 14, `upsert` 6,
`without_rowid` 7, `conflict` 19, `statement_lifecycle` 41.
The product's own `without_rowid` target (crates/amphora), on 3a8a80a0: 17 passed (r2095).
Not run: macOS (queued the whole hour, stopped before it started), the suites that need the `simulator` feature, the corpus, the gate.

## 7. Reproduce

```sh
python3 bench/sqlite_trigger_conflicts.py 250 C22 > crates/engine/tests/fixtures/trigger_conflicts.tsv   # as committed; 900 wide3: wider
cargo test -p amphora-engine --locked --test windows_and_triggers
```

Lost time: two runs refused or lost by kiln (0 of 32 cores idle; a server that didn't answer, and a mutants run it forgot, started again). Scratch refs on GitHub, never merged: perf/dml-c22, -land, -red, -probe-base (the probe, the wide draws, the mutants).
