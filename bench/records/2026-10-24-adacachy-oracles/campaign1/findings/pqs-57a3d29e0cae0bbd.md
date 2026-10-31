# pqs Wrong: the pivot (rowid 2), whose predicate SQLite rectified to TRUE, is missing

seed 11, state 766

## Statements

```sql
CREATE TABLE t0(c0  UNIQUE, c1 BLOB PRIMARY KEY);
INSERT INTO t0 VALUES (9223372036854775807, x'00');
INSERT INTO t0 VALUES ('a ', x'');
```

## Queries

```sql
SELECT ((a0.c1 NOT GLOB '*')) IS TRUE, ((a0.c1 NOT GLOB '*')) IS FALSE FROM t0 AS a0 WHERE a0.rowid = 2;
SELECT a0.rowid FROM t0 AS a0 WHERE (NOT (a0.c1 NOT GLOB '*'));
```

## What differed

```
the pivot (rowid 2), whose predicate SQLite rectified to TRUE, is missing
  Amphora: 0 rows
```
