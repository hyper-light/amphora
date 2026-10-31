# diff Wrong: Amphora's answer isn't SQLite's

seed 11, state 646

## Statements

```sql
CREATE TABLE t0(c0 REAL, c1 BLOB PRIMARY KEY);
INSERT OR IGNORE INTO t0 VALUES (100, x'');
```

## Queries

```sql
SELECT pi() FROM t0 AS a0, t0 AS a1 WHERE (a1.c1 GLOB '*');
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    3.141592653589793
  Amphora:  0 rows
```
