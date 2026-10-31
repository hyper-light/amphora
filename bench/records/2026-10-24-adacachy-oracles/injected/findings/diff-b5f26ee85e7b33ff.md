# diff Wrong: Amphora's answer isn't SQLite's

seed 20261024, state 738

## Statements

```sql
CREATE TABLE t0(c0 BOOLEAN NOT NULL, c1 TEXT PRIMARY KEY);
CREATE TABLE t2(c0 INTEGER, c1 BLOB COLLATE RTRIM);
INSERT INTO t2 VALUES (255, 10);
```

## Queries

```sql
SELECT 3.0 FROM t2 AS a0 FULL JOIN t0 AS a1 ON -9223372036854775808 WHERE (a1.c0 ISNULL);
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    3.0
  Amphora:  0 rows
```
