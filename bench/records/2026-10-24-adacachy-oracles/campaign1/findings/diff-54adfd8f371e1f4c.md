# diff Wrong: Amphora's answer isn't SQLite's

seed 11, state 849

## Statements

```sql
CREATE TABLE t1(c0 BOOLEAN NOT NULL, c1 INTEGER, c2 VARCHAR(10), c3 VARCHAR(10));
INSERT INTO t1 VALUES ('1e3', NULL, x'41', 2);
```

## Queries

```sql
SELECT (a0.c2 LIKE '%') FROM t1 AS a0;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    1
  Amphora:  1 rows
    0
```
