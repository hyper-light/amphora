# diff Wrong: Amphora's answer isn't SQLite's

seed 13, state 13360

## Statements

```sql
CREATE TABLE t0(c0 , c1 INTEGER);
INSERT INTO t0 VALUES ('1', -2.5);
```

## Queries

```sql
SELECT '10', NULL FROM t0 AS a0 JOIN t0 AS a1 ON (CASE a0.c0 WHEN a1.c1 THEN -1 WHEN 1 THEN 1 ELSE 'x' END) GROUP BY '10';
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 0 rows
  Amphora:  1 rows
    '10', NULL
```
