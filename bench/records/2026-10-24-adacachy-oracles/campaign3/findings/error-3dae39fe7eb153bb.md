# pqs Error: Amphora errors where SQLite finds the pivot: json_replace() needs an odd number of arguments

seed 13, state 7707

## Statements

```sql
CREATE TABLE t0(c0 TEXT, c1 INTEGER);
INSERT OR IGNORE INTO t0 VALUES (3, 0.0);
INSERT OR IGNORE INTO t0 VALUES (3.0, x'c3bf'), (1, 0.1), (' a', NULL);
```

## Queries

```sql
SELECT ((NULL == (CASE WHEN NULL THEN '{"a":1,"b":[2,3]}' WHEN (NULL NOT IN (SELECT s850.c1 FROM t0 AS s850 WHERE a0.c0)) THEN json_replace(NULL, NULL) END))) IS TRUE, ((NULL == (CASE WHEN NULL THEN '{"a":1,"b":[2,3]}' WHEN (NULL NOT IN (SELECT s850.c1 FROM t0 AS s850 WHERE a0.c0)) THEN json_replace(NULL, NULL) END))) IS FALSE FROM t0 AS a0 WHERE a0.rowid = 4;
SELECT a0.rowid FROM t0 AS a0 WHERE ((NULL == (CASE WHEN NULL THEN '{"a":1,"b":[2,3]}' WHEN (NULL NOT IN (SELECT s850.c1 FROM t0 AS s850 WHERE a0.c0)) THEN json_replace(NULL, NULL) END)) ISNULL);
```

## What differed

```
Amphora errors where SQLite finds the pivot: json_replace() needs an odd number of arguments
```
