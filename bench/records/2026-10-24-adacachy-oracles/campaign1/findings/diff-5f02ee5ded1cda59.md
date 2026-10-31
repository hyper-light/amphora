# diff Wrong: Amphora's answer isn't SQLite's

seed 11, state 593

## Statements

```sql
CREATE TABLE t0(c0 REAL COLLATE NOCASE NOT NULL, c1 BOOLEAN PRIMARY KEY NOT NULL, c2 REAL);
INSERT INTO t0 VALUES (-1e-308, -2.5, x'41');
CREATE TABLE t1(c0 INTEGER PRIMARY KEY NOT NULL, c1 TEXT);
INSERT INTO t1 VALUES (NULL, 0);
```

## Queries

```sql
SELECT 'a ' FROM t0 AS a0 JOIN t1 AS a2 ON (a0.c2 LIKE 'A%');
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    'a '
  Amphora:  0 rows
```
