# diff Wrong: Amphora's answer isn't SQLite's

seed 11, state 795

## Statements

```sql
CREATE TABLE t0(c0 DECIMAL(10,2));
INSERT INTO t0 VALUES (x'0102');
CREATE TABLE t2(c0 NUMERIC UNIQUE);
INSERT OR IGNORE INTO t2 VALUES (10), (x'0102');
```

## Queries

```sql
SELECT glob((CASE WHEN ('0x10' NOT IN (a1.c0, x'41')) THEN a0.c0 WHEN (-2.5 IS DISTINCT FROM a0.c0) THEN a1.c0 ELSE a2.c0 END), a2.c0) FROM t0 AS a0 FULL JOIN t2 AS a1 ON (a1.c0 >= (1.5 % a1.c0)), t2 AS a2;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 4 rows
    0
    1
    NULL
    NULL
  Amphora:  4 rows
    0
    0
    NULL
    NULL
```
