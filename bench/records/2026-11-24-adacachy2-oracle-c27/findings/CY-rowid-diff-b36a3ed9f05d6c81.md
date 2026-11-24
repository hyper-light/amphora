# diff Wrong: Amphora's answer isn't SQLite's

seed 27004, state 16594

## Statements

```sql
CREATE TABLE t0(c0 INTEGER COLLATE NOCASE PRIMARY KEY) STRICT;
CREATE TABLE t1(c0 DOUBLE);
INSERT OR REPLACE INTO t1 VALUES ('a');
INSERT INTO t1 VALUES ('A');
```

## Queries

```sql
SELECT a0.c0, 7, NULL FROM t0 AS a0 UNION SELECT a0.c0, NULL, NULL FROM t1 AS a0 GROUP BY a0.c0;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 2 rows
    'A', NULL, NULL
    'a', NULL, NULL
  Amphora:  1 rows
    'a', NULL, NULL
```
