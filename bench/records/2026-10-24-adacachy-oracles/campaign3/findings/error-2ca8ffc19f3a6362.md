# pqs Error: Amphora errors where SQLite finds the pivot: Parse error: Unexpected character

seed 13, state 1757

## Statements

```sql
CREATE TABLE t0(c0  COLLATE RTRIM, c1 VARCHAR(10) COLLATE RTRIM PRIMARY KEY);
INSERT INTO t0 VALUES (-9223372036854775808, x'c3bf');
```

## Queries

```sql
SELECT (((json_pretty(a0.c1, 10) IN (SELECT s477.c0 FROM t0 AS s477)) OR (NULL ISNULL))) IS TRUE, (((json_pretty(a0.c1, 10) IN (SELECT s477.c0 FROM t0 AS s477)) OR (NULL ISNULL))) IS FALSE FROM t0 AS a0 WHERE a0.rowid = 1;
SELECT a0.rowid FROM t0 AS a0 WHERE ((json_pretty(a0.c1, 10) IN (SELECT s477.c0 FROM t0 AS s477)) OR (NULL ISNULL));
```

## What differed

```
Amphora errors where SQLite finds the pivot: Parse error: Unexpected character
```
