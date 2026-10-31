# diff Wrong: Amphora's answer isn't SQLite's

seed 12, state 1010

## Statements

```sql
CREATE TABLE t0(c0 DOUBLE COLLATE NOCASE, c1 NUMERIC);
INSERT OR IGNORE INTO t0 VALUES (x'00', -2.5);
```

## Queries

```sql
SELECT jsonb_array((a0.c0 || 0), -1e-308) FROM t0 AS a0;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    x'cb12785c753030303030952d312e30652d333038'
  Amphora:  1 rows
    x'cb0d290030952d312e30652d333038'
```
