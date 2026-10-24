# diff Wrong: Amphora's answer isn't SQLite's

seed 11, state 241

## Statements

```sql
CREATE TABLE t0(c0 INTEGER, c1 REAL, c2 BOOLEAN NOT NULL);
INSERT INTO t0 VALUES (x'41', '%', 2);
```

## Queries

```sql
SELECT a1.c2 FROM t0 AS a0 LEFT JOIN t0 AS a1 ON (a1.c0 LIKE 'a%');
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    2
  Amphora:  1 rows
    NULL
```
