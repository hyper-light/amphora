# diff Wrong: Amphora's answer isn't SQLite's

seed 13, state 22971

## Statements

```sql
CREATE TABLE t0(c0 DECIMAL(10,2) COLLATE RTRIM, c1 TEXT);
INSERT OR IGNORE INTO t0 VALUES (NULL, x'00');
CREATE TABLE t1(c0 NUMERIC COLLATE NOCASE UNIQUE, c1 TEXT);
INSERT OR IGNORE INTO t1 VALUES ('A', 3);
CREATE TABLE t2(c0 BOOLEAN, c1 DOUBLE, c2 BLOB, c3  COLLATE BINARY);
INSERT OR IGNORE INTO t2 VALUES (x'0102', 1, -9223372036854775808, -9223372036854775808);
```

## Queries

```sql
SELECT (a2.c0 NOTNULL) FROM t2 AS a0 FULL JOIN t1 AS a1 ON ('a' BETWEEN CAST(a1.c1 AS INTEGER) AND (CASE a0.c3 WHEN (a1.c1 IS NOT a0.c3) THEN x'0102' ELSE a1.c0 END)), t0 AS a2;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 2 rows
    0
    0
  Amphora:  1 rows
    0
```
