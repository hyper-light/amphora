# pqs Error: Amphora errors where SQLite finds the pivot: JSON path error near: "2"

seed 27007, state 3307

## Statements

```sql
CREATE TABLE t0(c0 VARCHAR(10));
CREATE TABLE t1(c0 TEXT COLLATE NOCASE, c1 NUMERIC PRIMARY KEY, c2 NUMERIC COLLATE BINARY);
INSERT OR IGNORE INTO t1 VALUES (2.0, 9007199254740993, 0.5), (0.0, 1000000000000000.0, 'A');
INSERT OR IGNORE INTO t1 VALUES (2.0, 2.0, 9223372036854775807);
```

## Queries

```sql
SELECT ((NULL IN (SELECT s106.c0 FROM t0 AS s106 WHERE (json_set(a0.c1, a0.c1, x'c3bf') BETWEEN CAST(2.0 AS REAL) AND s106.c0)))) IS TRUE, ((NULL IN (SELECT s106.c0 FROM t0 AS s106 WHERE (json_set(a0.c1, a0.c1, x'c3bf') BETWEEN CAST(2.0 AS REAL) AND s106.c0)))) IS FALSE FROM t1 AS a0 WHERE a0.rowid = 3;
SELECT a0.rowid FROM t1 AS a0 WHERE (NOT (NULL IN (SELECT s106.c0 FROM t0 AS s106 WHERE (json_set(a0.c1, a0.c1, x'c3bf') BETWEEN CAST(2.0 AS REAL) AND s106.c0))));
```

## What differed

```
Amphora errors where SQLite finds the pivot: JSON path error near: "2"
```
