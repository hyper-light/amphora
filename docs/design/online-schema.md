# Online schema changes by F1's states (CP-T1.12)

Requirement: CP-T1.12 (AC-T1.12.1, AC-T1.12.2; T-T1.12.1). Kiln goal 496de8d5, G14 step 3.

## Decision (lane online-schema): an index is built absent, delete-only, write-only, public

`Database::change_schema` builds the index one `CREATE [UNIQUE] INDEX` names without holding the
writer for its table's size. Each step is one transaction through the writer (`Database::call`), its
effect and its record (`__amphora_schema_changes`) committed together:

1. **Absent to delete-only.** The index's name is staged in `__amphora_index_states`, then `CREATE
   INDEX` makes the index under it empty. Deletes remove its entries; inserts add none.
2. **Delete-only to write-only.** Writes keep it, unique checks included. The change records the
   table's largest rowid: every row after it is written with its entry.
3. **Backfill.** Batches give the rows up to that rowid their entries (`Connection::prepare_index_backfill`),
   in rowid order, skipping a row that has one. A row another row's entry already holds under a
   unique key stops the change, and it is undone (its index dropped): the old schema, refused typed.
4. **Write-only to public, the cutover.** The state row goes; only the catalog changes, whatever the
   table's size. The migration is recorded in `__amphora_migrations` in the same transaction.

The engine keeps each index's state on `Index::state`, read from `__amphora_index_states` whenever a
connection parses its schema (a reparse phase beside the custom types', and the same read when a
database opens, whose first schema the engine builds by another path), so the writer and every
reader see one state at one schema cookie, and a state change moves the cookie. Only a public index
is read: the planner's candidates, foreign-key parent lookups, UPSERT targets, ANALYZE and the
integrity check's entry comparison all skip the others. At run time an insert into a delete-only
index does nothing, and a delete that finds no entry in a non-public index is expected. An index or
table held by a change refuses `DROP`.

## Decision (lane online-schema-5): a column is added with its states run together

`Database::change_schema` also takes one `ALTER TABLE … ADD COLUMN`. Adding a column rewrites no
row: a row written before it reads the column's constant default, and the engine's checks of the
addition read at most the table's first row (is it empty: NOT NULL with a NULL default, a
non-constant default, a STRICT default's type) and the default itself (its CHECK). So F1's absent,
delete-only, write-only and public for a column (section 4 of the paper, where delete-only and
write-only exist so servers on different schemas agree while the column is backfilled) run together
here: there is nothing to backfill, and with one writer the drain is the commit. The change is one
transaction, its record in `__amphora_migrations` in it: a crash leaves the table without the column
or with it. A column whose addition reads every row is refused: a generated column with NOT NULL or
CHECK, which `ALTER TABLE` checks against each row (`emit_column_constraints_validation`); so are
the other `ALTER TABLE` forms (DROP COLUMN, ALTER COLUMN and RENAME rewrite or check every row or
every statement naming the table). What would falsify it: an `ADD COLUMN` that the engine runs with
a scan of the table (its program loops over the table's rows), which `t_t1_12_1_a_column_is_added_in_one_catalog_change_and_its_constraints_hold_from_it`
would not see; read `translate/alter.rs`'s `AddColumn` arm when it changes.

## Decision (lane online-schema-6): a constraint is added write-only, validated, then public

`Database::change_schema` takes one `ALTER TABLE t ALTER COLUMN c TO <def>` whose `<def>` is `c`'s
own definition (same name, type and constraints, in order) with NOT NULL or CHECK constraints after
it; anything else is refused before anything changes, so the change can always be undone by
defining the column as it was. F1's states for a constraint (section 4: write-only enforces it on
new writes, the reorganization validates the old rows, then public):

1. **Absent to write-only**, one transaction: a row of `__amphora_constraint_states` (table, column
   and the old definition) marks the column unvalidated, then `ALTER COLUMN` redefines it. Every
   write from here is checked against the constraint. The engine reads the mark whenever a
   connection parses its schema (a reparse phase after the index states', and on open), sets
   `Column::unvalidated`, and then: `ALTER COLUMN` on that column doesn't scan the table (the
   batches do), the planner doesn't rely on its NOT NULL (`trusted_notnull` in
   `optimizer/cardinality.rs` and `Expr::is_nonnull`), and the integrity check doesn't call a NULL
   in it corrupt. The change records the table's largest rowid: rows after it were written checked.
2. **Validation**: batches read the rows up to that rowid in rowid order, at most the batch's rows,
   each checked for a NULL under NOT NULL or a CHECK that is false (a NULL CHECK passes, as
   SQLite's does). Batches are sized by the same learned `Sizer` as an index's backfill. A breaking
   row stops the change (`online::BROKEN`) and it is undone: the column redefined as it was (its
   mark still on, so the old constraints, which every row keeps, are not scanned for), then the mark
   and the record removed, in one transaction.
3. **Public, the cutover**: the mark goes, and the migration is recorded, in one transaction: only
   the catalog changes, whatever the table's size.

A power cut leaves the old schema (no constraint, or the constraint marked unvalidated and its
migration unrecorded) or the new (declared, unmarked, recorded), never a public constraint without
its record: `crates/amphora/tests/crash.rs`'s
`t_t1_12_1_a_power_cut_at_every_step_of_an_online_constraint_leaves_old_or_new`. Refused: a
generated column (its constraint is checked by computing every row at once), a WITHOUT ROWID table
(no rowid order). What would falsify it: a write between two batches that leaves a row breaking the
constraint at or below the recorded rowid after its batch passed (writes are checked from step 1,
so it would mean the engine skipped the check for an unvalidated column), or a plan that answers
`c IS NULL` from the declaration while the mark is on (`t_t1_12_1_an_unvalidated_not_null_is_not_relied_on_and_a_breaking_row_undoes_the_change`).

**Sources.** Rae, Rollins, Shute, Sodhi, Vingralek, "Online, Asynchronous Schema Change in F1",
PVLDB 6(11), 2013 (https://www.vldb.org/pvldb/vol6/p1045-rae.pdf): section 4's states (absent,
delete-only, write-only, public), the reorganization (backfill) in write-only, and at most two
schema versions in use at once. Kiln research 01a16b0f47d5716eb35fedb0ee9c79b4, finding f4.

**F1's drain, here.** F1 waits for every server to reach a state before the next, as servers run
different schemas. Here every write runs on the writer's connection, and a statement prepared under
an older cookie is prepared again before it runs, so once a step commits no write runs on the state
before it: the drain is the commit. What would falsify it: a writer that runs transactions
concurrently (the multi-writer engine of `engine.md`) — then a step must wait for every transaction
begun under the state before it to end, as F1 and PostgreSQL's `CREATE INDEX CONCURRENTLY` wait.

**Crash safety (T-T1.12.1).** A step's effect and record commit together, so a crash leaves the
change at a step. Until public, no read uses the index: the schema readers see is the old one. A
public index is recorded as migrated in the same transaction. Calling again with the same
migration carries the change on. `crates/amphora/tests/crash.rs`
(`t_t1_12_1_a_power_cut_at_every_step_of_an_online_index_leaves_old_or_new`) cuts the power at
every change the build makes, restarts, and checks old or new, never both or neither, then carries
the change to its end.

**Batch size, learned as it runs.** No size is picked. The first batch is a row; each next is the
last's rows scaled by the time the last waited (for the writer and its flush: the call's round trip
less its own hold) over the time it held the writer, at most twice the last's, at least one row;
too many dirty pages for one transaction halves it. So a batch holds the writer about as long as it
waited for it: the backfill takes at most about half the writer's time, under any load and on any
machine, and grows as fast as the writer lets it. What would falsify it: write p99 during a backfill
more than twice its p99 without one, measured by bench `online_ddl`.

**Cutover pause (AC-T1.12.2).** The cutover changes the catalog and reparses the writer's schema:
its cost grows with the schema's size, not the table's. `SchemaChange::cutover` says how long it held
the writer; the 10,000,000-row measurement is the coordinator's.

**Left.** Foreign keys and a generated column's constraints added online; a dropped column's
slot freed of its name (PostgreSQL renames it), and its bytes reclaimed by a rewrite; a drop on a
table with triggers or WITHOUT ROWID; indexes on WITHOUT ROWID
tables (no rowid order to backfill by: refused typed); routing `migrate`'s index builds on large
tables here; undoing a big index drops its pages in one transaction; metrics, to be agreed with
Kiln Metrics.

## Decision (lane online-schema-7): a column is dropped as a hidden slot, cleared in batches

`Database::change_schema` takes one `ALTER TABLE t DROP COLUMN c`. A row can't be rewritten to a
new layout in batches while the rows not yet reached keep the old one in the same table, so the
column stays as a slot, as PostgreSQL's `pg_attribute.attisdropped` keeps a dropped column
(PostgreSQL documentation, ALTER TABLE: "The DROP COLUMN form does not physically remove the
column, but simply makes it invisible to SQL operations. Subsequent insert and update operations
in the table will store a null value for the column",
https://www.postgresql.org/docs/current/sql-altertable.html, Notes). F1's states for removing a column (public,
write-only, delete-only, absent: section 4, run in reverse to add's) run together in the first
step for the same reason they do for `ADD COLUMN`: with one writer the drain is the commit, and
nothing must be read before the column goes. The steps, each one transaction through the writer:

1. **The switch-over**, a catalog change: a row of `__amphora_dropped_columns` (table, column)
   names the slot, read whenever a connection parses its schema (a reparse phase after the
   constraint states', and on open) and set on the column as `Column::dropped`. From this commit
   no name finds it (`BTreeTable::get_column` and the binder's qualified lookups pass it), `*`, an
   insert without a column list and `PRAGMA table_info`/`table_xinfo` pass it (it is hidden), an
   insert or update naming it is refused as naming no column, and every insert and update stores
   NULL in it: its NOT NULL and its own CHECK are off on the engine's copy, and an update writes
   NULL where it would carry the old value. The change records the table's largest rowid.
2. **The clear**: batches rewrite the rows up to that rowid in rowid order, at most the batch's
   rows, each by setting a plain column to itself, which stores NULL in the slot. Batches are
   sized by the same learned `Sizer` as an index's backfill; too many dirty pages halves one.
3. **The record**: the change's row goes and the migration is recorded, in one transaction.

The slot's row stays: the slot is the table's for good, and keeps its name (an `ADD COLUMN` or a
rename to it is refused as a duplicate), as the `CREATE TABLE` text in `sqlite_schema` keeps the
column for the rows' layout. A drop has no breaking row, so once begun it only goes on: a power cut
leaves the column found by name and unrecorded (old) or hidden (new), unrecorded while the clear is
under way and recorded once it ends, never found by name with its drop recorded:
`crates/amphora/tests/crash.rs`'s `t_t1_12_1_a_power_cut_at_every_step_of_an_online_drop_leaves_old_or_new`.

Refused before anything changes: whatever the engine's own `DROP COLUMN` refuses (its statement is
prepared and never run: a key, UNIQUE, an index, a CHECK or generated column naming it, a foreign
key, a view or a trigger naming it, the only column), a WITHOUT ROWID table (no rowid order to clear
by), a table with triggers (the clear's updates would fire them), and a table with no other plain
column (not generated, not a key) for the clear to rewrite rows by. What would falsify it: a query,
an insert or an update that reads or writes the slot by name after the switch-over, or a row whose
slot holds a value after the record (`t_t1_12_1_a_dropped_column_is_hidden_while_its_values_are_cleared_then_recorded`
reopens the database without the mark and counts them).
