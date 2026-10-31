# pqs Wrong: the pivot (rowid 8), whose predicate SQLite rectified to TRUE, is missing

seed 20261024, state 815

## Statements

```sql
CREATE TABLE t0(c0 TEXT, c1 NUMERIC, c2 DOUBLE COLLATE NOCASE PRIMARY KEY, c3  COLLATE BINARY);
INSERT INTO t0 VALUES (3, 100, 'x', '0');
INSERT OR IGNORE INTO t0 VALUES (-1, 3.0, 0, '1'), (2, 1, 2, NULL);
INSERT OR IGNORE INTO t0 VALUES (' ', 'abc', '1', 1000000000000000.0), (2, 9007199254740993, 'a', ' a');
INSERT OR IGNORE INTO t0 VALUES (100, '1.5', NULL, 2), (1000000000000000.0, 9007199254740993, '0x10', 100), (x'', x'0102', 1e308, ' a');
```

## Queries

```sql
SELECT ((NULL AND a0.c1)) IS TRUE, ((NULL AND a0.c1)) IS FALSE FROM t0 AS a0 WHERE a0.rowid = 8;
SELECT a0.rowid FROM t0 AS a0 WHERE (NOT (NULL AND a0.c1));
```

## What differed

```
the pivot (rowid 8), whose predicate SQLite rectified to TRUE, is missing
  Amphora: 0 rows
```
