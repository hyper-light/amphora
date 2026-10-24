# pqs Wrong: the pivot (rowid 1), whose predicate SQLite rectified to TRUE, is missing

seed 20261024, state 1203

## Statements

```sql
CREATE TABLE t0(c0 INT, c1 INT, c2 VARCHAR(10) NOT NULL, c3 BLOB COLLATE BINARY PRIMARY KEY);
INSERT OR IGNORE INTO t0 VALUES ('1e3', NULL, -1, x'');
```

## Queries

```sql
SELECT ((NULL AND (a0.c2 ISNULL))) IS TRUE, ((NULL AND (a0.c2 ISNULL))) IS FALSE FROM t0 AS a0 WHERE a0.rowid = 1;
SELECT a0.rowid FROM t0 AS a0 WHERE (NOT (NULL AND (a0.c2 ISNULL)));
```

## What differed

```
the pivot (rowid 1), whose predicate SQLite rectified to TRUE, is missing
  Amphora: 0 rows
```
