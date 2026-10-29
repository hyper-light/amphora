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
(generated columns, user triggers, UPDATE FROM, CDC, and custom-type or array
mutations/indexes) retain explicit refusals;
this change does not claim support for those combinations.
