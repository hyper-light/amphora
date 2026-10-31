# diff Wrong: Amphora's answer isn't SQLite's

seed 20261028, state 75814

## Statements

```sql
CREATE TABLE t0(c0 INT, c1 BOOLEAN);
INSERT INTO t0 VALUES (3.0, 'A');
INSERT INTO t0 VALUES (-1e-308, 0);
```

## Queries

```sql
SELECT abs((0 / a0.c0)) FROM t0 AS a0;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 2 rows
    0
    -0.0
  Amphora:  2 rows
    0
    0.0
```
