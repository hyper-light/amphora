# The correctness corpus, run on SQLite

**114,556 distinct query types** (a statement's shape with the inventory entries it exercises, keywords aside); **570 of 570 inventoried entries (100.0%) exercised by at least 10 distinct statements**. Counted coarser (a statement kind with the syntax, functions, operators, PRAGMAs and date modifiers it uses), 56,691 types.

| source | files | records kept | answers compared | agree | disagree | query types | of them only here | entries reached |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| slt | 622 | 5,939,879 | 5,939,879 | 5,939,878 | 1 | 106,076 | 105,760 | 145 |
| duckdb | 5,836 | 19,805 | 6,705 | 4,365 | 2,340 | 3,448 | 2,549 | 325 |
| cockroach | 610 | 14,198 | 2,937 | 1,376 | 1,561 | 2,235 | 1,548 | 330 |
| postgres | 240 | 13,178 | 13,144 | 9,571 | 3,573 | 1,415 | 818 | 302 |
| kiln | 1 | 507 | 507 | 507 | 0 | 247 | 183 | 146 |
| written | 470 | 8,942 | 8,942 | 8,942 | 0 | 2,947 | 2,507 | 570 |

For slt, kiln and written, `agree` is SQLite's answer against the one the file carries (sqllogictest's own; SQLite's for the others, so those agreeing say the files replay). For the translated sources (duckdb, cockroach, postgres) it's SQLite's answer against the source engine's: their records are what was read, what sqlglot couldn't translate, what SQLite refused, and what was kept. A record both engines refuse is kept as `statement error` (an error path is a case too): the few hundred query types under statement kinds SQLite doesn't have (COPY, CALL, PREPARE) are those.

| source | records | untranslatable | SQLite refused | kept | kept partly | not compared | files left out |
|---|---:|---:|---:|---:|---:|---:|---:|
| duckdb | 45,154 | 3,991 | 21,323 | 19,805 | 12 | 45 | 2,765 |
| cockroach | 53,512 | 7,694 | 31,612 | 14,198 | 398 | 241 | 0 |
| postgres | 53,671 | 6,777 | 33,713 | 13,178 | 1 | 0 | 0 |

## Entries by kind

| kind | entries | reached by 10+ | least | median |
|---|---:|---:|---:|---:|
| syntax | 71 | 71 | 12 | 409+ |
| function.core | 51 | 51 | 10 | 23 |
| function.aggregate | 12 | 12 | 10 | 120 |
| function.window | 11 | 11 | 23 | 56 |
| function.datetime | 7 | 7 | 12 | 24 |
| function.math | 29 | 29 | 10 | 16 |
| function.json | 34 | 34 | 10 | 12 |
| datetime.modifier | 19 | 19 | 10 | 10 |
| pragma | 74 | 74 | 10 | 11 |
| operator | 51 | 51 | 10 | 287+ |
| parameter | 5 | 5 | 11 | 21 |
| expression | 4 | 4 | 124 | 540+ |
| affinity | 5 | 5 | 145 | 783+ |
| collation | 3 | 3 | 38 | 47 |
| storage_class | 5 | 5 | 147 | 1213+ |
| rule.affinity | 6 | 6 | 29 | 828+ |
| rule.expression-affinity | 8 | 8 | 10 | 10 |
| rule.comparison | 11 | 11 | 12 | 12 |
| rule.sort | 3 | 3 | 10 | 10 |
| rule.collation | 7 | 7 | 10 | 10 |
| keyword | 147 | 147 | 10 | 286+ |
| word | 7 | 7 | 10 | 174 |

Distinct statements an entry are counted up to 256 a source (256+ means at least that many).

## Query types by statement kind

| kind | query types |
|---|---:|
| SELECT | 110,611 |
| CREATE TABLE | 743 |
| CREATE VIEW | 694 |
| DELETE | 588 |
| INSERT | 400 |
| PRAGMA | 270 |
| WITH | 228 |
| UPDATE | 218 |
| ALTER | 131 |
| CREATE INDEX | 54 |
| BEGIN | 44 |
| COPY | 26 |
| CREATE TRIGGER | 25 |
| SET | 24 |
| CALL | 24 |
| CREATE FUNCTION | 18 |
| DROP TABLE | 16 |
| PREPARE | 16 |
| ( | 16 |
| EXECUTE | 15 |
| CREATE OR | 14 |
| VALUES | 14 |
| CREATE STATISTICS | 14 |
| CREATE MACRO | 12 |
| ROLLBACK | 10 |

## How it ran

SQLite 3.54.0 the oracle (Python 3.14 sqlite3, the system libsqlite3 (compile options in the run's record)); translation by sqlglot 30.24.0. Each file runs on a fresh database file as slt_sqlite.c runs it: synchronous off; an I column is sqlite3_column_int (32 bits), an R column SQLite's own printf("%.3f"), a T column the text's bytes up to the first NUL with each outside ' '..'~' an @; rowsort and valuesort as its runner sorts. Each file in a process of its own (no process-wide setting reaches the next), within budgets: 20 s and a million rows a record, 1.5 GiB a process. A query answering from random() or the clock is kept as a statement (run, its answer not checked).

| source | repository | revision | read |
|---|---|---|---|
| slt | https://github.com/gregrahn/sqllogictest.git | c93ac416f056 | test/**/*.test, all 622 files (evidence, index, random, select1-5), whole |
| duckdb | https://github.com/duckdb/duckdb.git | 1641475ef1db | test/sql/**/*.test and *.test_slow; a file is left out whole when it needs DuckDB's runner (require, load, restart, reconnect, loop, foreach, concurrentloop, mode, attach, unzip, test-env, tags) |
| cockroach | https://github.com/cockroachdb/cockroach.git | d30c905fff79 | pkg/sql/logictest/testdata/logic_test/* |
| postgres | https://github.com/postgres/postgres.git | 126976a8ffcf | src/test/regress/sql/*.sql except test_setup.sql, on the suite's base tables (test_setup.sql and data/*.data), written once as postgres/base.test, the fixture each file runs first |
| kiln | https://github.com/hyper-light/kiln.git | 8adeadba572e | every SQL string literal in crates/** at that revision (tests and vendor left out), and crates/kiln/src/store/migrations/*.sql, its production schema |
| written | this one |  | bench/corpus/written_cases.py |

## Every expected result SQLite's own

Each translated source's files run back on SQLite (`build_corpus.py replay-<source>`): the answers they carry are the ones SQLite gives. A query whose answer moved between the run that wrote it and its replay answers from the clock or chance through its data (PostgreSQL's timestamp tests store 'now', which SQLite's date functions read as the time): `settle.py` keeps it as a statement and the replay runs again.

| source | files | statements | queries | agree | disagree | queries settled |
|---|---:|---:|---:|---:|---:|---:|
| duckdb | 2,328 | 13,116 | 6,712 | 19,828 | 0 | 0 |
| cockroach | 496 | 11,997 | 3,092 | 15,089 | 0 | 0 |
| postgres | 206 | 8,077 | 5,134 | 13,211 | 0 | 6 |

## Disagreements, triaged

For the translated sources, SQLite's answer against the source engine's own (DuckDB's and CockroachDB's expected results, PostgreSQL 17 run live), each counted under the first cause that fits it (`translated.classify`).

### duckdb

| cause | count | what | example |
|---|---:|---|---|
| semantics | 915 | the same SQL means something else on SQLite (affinity, NULL and type rules, a function's own semantics) | `test/sql/aggregate/aggregates/test_aggregate_types_scalar.test:56` |
| type | 508 | a type SQLite doesn't have (wide and unsigned integers, DECIMAL, intervals, time zones, UUID, BIT, ENUM, nested types): SQLite reads the value by affinity | `test/sql/aggregate/aggregate_state_export/aggregate_state_union.test:14` |
| order | 332 | the same rows in another order (no ORDER BY, or ties under it) | `test/sql/aggregate/aggregates/test_aggr_string.test:8` |
| rows | 261 | a row count from data the translation couldn't carry: rows a COPY from a file, a statement SQLite refused, or another session made | `test/sql/aggregate/aggregates/test_bitstring_agg.test:106` |
| datetime | 209 | dates and times: SQLite keeps them as text or numbers, with functions of its own | `test/sql/aggregate/aggregates/test_aggregate_types_scalar.test:47` |
| plan | 98 | a plan (EXPLAIN): each engine's own | `test/sql/binder/test_duplicate_in_set_operation.test:34` |
| catalog | 17 | the engine's own catalog (PRAGMAs, information_schema, pg_catalog, duckdb_*, crdb_internal, SHOW) | `test/sql/attach/attach_table_info.test:12` |

### cockroach

| cause | count | what | example |
|---|---:|---|---|
| semantics | 786 | the same SQL means something else on SQLite (affinity, NULL and type rules, a function's own semantics) | `pkg/sql/logictest/testdata/logic_test/alter_column_type:15` |
| rows | 328 | a row count from data the translation couldn't carry: rows a COPY from a file, a statement SQLite refused, or another session made | `pkg/sql/logictest/testdata/logic_test/alter_primary_key:1079` |
| type | 288 | a type SQLite doesn't have (wide and unsigned integers, DECIMAL, intervals, time zones, UUID, BIT, ENUM, nested types): SQLite reads the value by affinity | `pkg/sql/logictest/testdata/logic_test/bit:1` |
| datetime | 122 | dates and times: SQLite keeps them as text or numbers, with functions of its own | `pkg/sql/logictest/testdata/logic_test/datetime:56` |
| order | 23 | the same rows in another order (no ORDER BY, or ties under it) | `pkg/sql/logictest/testdata/logic_test/alter_column_type:428` |
| catalog | 10 | the engine's own catalog (PRAGMAs, information_schema, pg_catalog, duckdb_*, crdb_internal, SHOW) | `pkg/sql/logictest/testdata/logic_test/pgoidtype:57` |
| division | 3 | division: the source divides integers into a fraction, SQLite into an integer | `pkg/sql/logictest/testdata/logic_test/vectorize:256` |
| plan | 1 | a plan (EXPLAIN): each engine's own | `pkg/sql/logictest/testdata/logic_test/partitioning_implicit:256` |

### postgres

| cause | count | what | example |
|---|---:|---|---|
| source refused | 1,734 | the source engine refuses what SQLite accepts (its type checks, its errors for overflow, division by zero, bad input) | `src/test/regress/sql/arrays.sql:180` |
| semantics | 1,027 | the same SQL means something else on SQLite (affinity, NULL and type rules, a function's own semantics) | `src/test/regress/sql/bit.sql:19` |
| type | 478 | a type SQLite doesn't have (wide and unsigned integers, DECIMAL, intervals, time zones, UUID, BIT, ENUM, nested types): SQLite reads the value by affinity | `src/test/regress/sql/bit.sql:62` |
| datetime | 211 | dates and times: SQLite keeps them as text or numbers, with functions of its own | `src/test/regress/sql/collate.linux.utf8.sql:180` |
| rows | 65 | a row count from data the translation couldn't carry: rows a COPY from a file, a statement SQLite refused, or another session made | `src/test/regress/sql/box.sql:152` |
| aborted transaction | 57 | PostgreSQL refused it only because an earlier error in the script aborted its transaction | `src/test/regress/sql/expressions.sql:199` |
| order | 1 | the same rows in another order (no ORDER BY, or ties under it) |  |

### slt

| cause | count | what | example |
|---|---:|---|---|
| the file's own unfinished record | 1 | the record is marked TBD-EVIDENCE-OF R-08904-24719: sum() over integers that overflow raises "integer overflow" (lang_aggfunc.html), and SQLite 3.54.0 does; the file expects an empty answer, what its generator wrote for a query that failed | `test/evidence/slt_lang_aggfunc.test:480` |
