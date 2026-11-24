# diff Wrong: Amphora's answer isn't SQLite's

seed 27007, state 6066

## Statements

```sql
CREATE TABLE t0(c0 TEXT COLLATE NOCASE, c1 REAL PRIMARY KEY);
INSERT INTO t0 VALUES (100, 7), (0.5, x'c3bf');
```

## Queries

```sql
SELECT 1, (7 IS a0.c0), NULL, count(NULL) OVER (ORDER BY NULL DESC) FROM t0 AS a0 GROUP BY 1, (7 IS a0.c0);
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
