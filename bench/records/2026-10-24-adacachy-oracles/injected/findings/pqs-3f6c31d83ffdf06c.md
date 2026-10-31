# pqs Wrong: the pivot (rowid 4), whose predicate SQLite rectified to TRUE, is missing

seed 20261024, state 591

## Statements

```sql
CREATE TABLE t0(c0 BOOLEAN, c1 DOUBLE);
INSERT OR IGNORE INTO t0 VALUES (-9223372036854775808, NULL), ('1.5', '1.5');
INSERT OR IGNORE INTO t0 VALUES ('_', 2.0), (NULL, -9223372036854775808);
```

## Queries

```sql
SELECT ((NULL AND (a0.c1 IS NOT a0.c1))) IS TRUE, ((NULL AND (a0.c1 IS NOT a0.c1))) IS FALSE FROM t0 AS a0 WHERE a0.rowid = 4;
SELECT a0.rowid FROM t0 AS a0 WHERE (NOT (NULL AND (a0.c1 IS NOT a0.c1)));
```

## What differed

```
the pivot (rowid 4), whose predicate SQLite rectified to TRUE, is missing
  Amphora: 0 rows
```
