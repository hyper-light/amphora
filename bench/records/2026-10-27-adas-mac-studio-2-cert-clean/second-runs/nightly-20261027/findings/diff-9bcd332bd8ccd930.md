# diff Wrong: Amphora's answer isn't SQLite's

seed 20261027, state 33282

## Statements

```sql
CREATE TABLE t0(c0 TEXT, c1 DOUBLE);
INSERT OR IGNORE INTO t0 VALUES (2.0, 3.0);
```

## Queries

```sql
SELECT NULL, NULL FROM t0 AS a0 WHERE (json_error_position(timediff(0, 0)) IN (NULL, a0.c1, 0.1)) GROUP BY NULL;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 0 rows
  Amphora:  1 rows
    NULL, NULL
```
