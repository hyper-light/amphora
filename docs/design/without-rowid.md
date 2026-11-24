# WITHOUT ROWID SQL compatibility

Requirement: AC-T1.1.1 and AC-T1.8.6. Kiln's Records and work migrations
use composite primary keys and must retain their declared storage semantics.

WITHOUT ROWID tables use their primary-key index as the table. Persistent
secondary indexes append missing primary-key terms as the row locator; declared
UNIQUE terms remain the uniqueness prefix. Collation-aware term matching avoids
losing a locator when the index and primary key use different collations. Reads
seek the table with that composite locator, and mutations maintain physical index
records rather than manufacturing integer rowids.

Sources: SQLite's [WITHOUT ROWID specification](https://www.sqlite.org/withoutrowid.html)
and [file format sections 2.4–2.5](https://www.sqlite.org/fileformat2.html).
These require primary-key NOT NULL semantics, primary-key-first table records,
and primary-key suffixes in secondary indexes. Removing the storage flag would
change those semantics and is not a compatibility solution.

Mutation planning snapshots primary keys before changing rows, then rereads each
row before evaluating its assignments. All assignments observe the original row;
correlated queries may observe earlier row updates. Replacement victims are
removed only after the candidate's constraints have passed. Foreign-key matching
uses the parent's affinity and collation, including generated action predicates,
as required by SQLite's [foreign-key specification](https://www.sqlite.org/foreignkeys.html).

UPSERT DO UPDATE uses ABORT even under an outer FAIL policy, following the
[UPSERT specification](https://www.sqlite.org/lang_upsert.html). The raw engine
retains statement-journal rollback. Product calls retain their existing stronger
failure boundary: a failed statement poisons the enclosing call, whose changes
cannot commit even if its closure catches the error (store.md, decision 7).

Evidence is the public `amphora` integration target `without_rowid`, the engine
integration target `without_rowid_dml`, and the physical-index metadata unit test
`without_rowid_secondary_locator_preserves_collation_and_unique_prefix`.
The production migration diagnostic ran all 110 current Kiln migrations against
the corrected engine, including Records' secondary feed index. The diagnostic
retains its existing special handling of the FTS5 virtual-table statement in
`20261018-2100_w19_log_search.sql`; it does not establish FTS5 support. This is
migration evidence, not a claim that every production query has been exercised.

The decision is falsified by a SQLite-oracle discrepancy in locator ordering,
collation, constraints, conflict actions, foreign-key effects, or rollback; by
index/table disagreement after reopen; or by a failed call publishing changes.
The focused suite includes those cases. Unsupported mutation combinations
(generated columns, UPDATE FROM, CDC, and custom-type or array
mutations/indexes) retain explicit refusals;
this change does not claim support for those combinations.

## Triggers

A trigger on a WITHOUT ROWID table fires as one on a rowid table does, by SQLite's
[CREATE TRIGGER](https://www.sqlite.org/lang_createtrigger.html) and its update.c, delete.c and
insert.c (3.54.0). Their row is the table's image registers (`without_rowid.rs`, `RowTriggers`);
it has no rowid to name, so `new.rowid` is \"no such column\".

- **UPDATE, and UPSERT's DO UPDATE.** NEW is converted to its columns' types, then the BEFORE
  triggers fire. They may have deleted the row or moved it to another key: the old key is sought
  again and such a row is done with. The row is read again, columns the statement doesn't set keep
  what the triggers wrote, and that image, not OLD, is the one the index entries are deleted by. The
  constraints are checked after that. AFTER triggers fire once the row, its foreign-key actions and
  RETURNING are done.
- **UPDATE OF** is asked by the columns the statement sets. The refusal this section replaces asked
  with none, and panicked on the first UPDATE OF trigger it met (the oracles' finding CP).
- **DELETE.** The BEFORE triggers fire, the key is sought again, the row is deleted, and the AFTER
  triggers fire with the row as it was read before the BEFORE triggers.
- **REPLACE** deletes rows without firing DELETE triggers, as recursive triggers are off
  ([conflict clauses](https://www.sqlite.org/lang_conflict.html)).
- **Conflict clauses.** A statement's own clause overrides its triggers'; a DO UPDATE's triggers run
  under ABORT; a DELETE's keep theirs. A foreign key's action is an UPDATE under ABORT, and so is
  what it fires (fkey.c, `sqlite3FkActions`). What a refused constraint does is its own Halt's
  (`HaltAction`), so each statement of a trigger's program has its own, and an error no clause is
  for (a rowid that is no integer, a function's) is under ABORT.
- **The key's place.** A conflict with the key is met in its place among the table's UNIQUE
  constraints, the newest first and those under REPLACE last (build.c, `sqlite3CreateIndex`): of two
  constraints in conflict, the first met is the one whose clause says what happens.

Evidence: `triggers_fire_as_sqlite_fires_them` and
`a_conflict_is_judged_by_the_clause_of_the_statement_that_meets_it`
(`crates/engine/tests/windows_and_triggers.rs`), generated sweeps of every event, time and clause on
rowid and WITHOUT ROWID tables against SQLite 3.54.0 (`bench/sqlite_triggers.py`,
`bench/sqlite_trigger_conflicts.py`). Falsified by any statement of it whose answer, refusal or rows
left behind differ, or by a failed `PRAGMA integrity_check` after one.

## Native primary-root query access

The planner offers the existing table root as a candidate using the same
primary-key descriptor as foreign-key resolution. Its terms contain only the
primary key: payload fields cannot become uniqueness or ordering terms. The
descriptor is not registered as a schema index and never enters index mutation
maintenance. FK-specific eligibility checks stay at their original boundary.

Payload projections retain the existing two-cursor path: the query seeks the
primary-root index cursor, reconstructs its composite key, and positions the
table cursor with `Found`. Table column emission uses the established logical
to physical mapping. This adds a bounded same-root lookup per matched row,
rather than introducing cursor aliases or integer rowid locators. Existing
secondary-index coverage and range/ordering rules remain unchanged. Comparisons
whose collation cannot use the descriptor retain their existing scan fallback.

Evidence: `native_primary_root_bounds_work_ledger_point_and_prefix_reads`
checks EXPLAIN and public statement counters on 20,000 work-ledger rows, with
four rows per weak key; `native_primary_root_preserves_collation_payload_and_join_fallbacks`
checks reordered keys, collations, direction, affinity, joins and fallback;
`native_primary_root_payload_and_mutations_survive_reopen` checks the durable
public boundary. These falsify the decision if an unrelated-row scan is required
for a supported leading-key lookup, payload/ordering changes, or secondary index
and mutation results disagree after reopen. Counter ceilings distinguish a
bounded lookup of at most four rows from visiting 20,000 unrelated rows; they
are correctness/access-shape checks, not latency claims.
