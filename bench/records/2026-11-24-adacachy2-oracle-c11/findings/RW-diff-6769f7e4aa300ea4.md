# diff Wrong: Amphora's answer isn't SQLite's

seed 8006, state 6228

## Statements

```sql
CREATE TABLE t0(c0 REAL COLLATE RTRIM PRIMARY KEY);
INSERT INTO t0 VALUES ('');
INSERT INTO t0 VALUES (-1e-308);
```

## Queries

```sql
SELECT 1, (a0.c0 << 'ABC'), NULL, count(NULL) OVER (ORDER BY NULL DESC) FROM t0 AS a0 GROUP BY 1, (a0.c0 << 'ABC');
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 2 rows
    1, 0, NULL, 0
    1, 0, NULL, 0
  Amphora:  1 rows
    1, 0, NULL, 0
```
