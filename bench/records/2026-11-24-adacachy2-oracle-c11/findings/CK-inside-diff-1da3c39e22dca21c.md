# diff Wrong: Amphora's answer isn't SQLite's

seed 8002, state 2662

## Statements

```sql
CREATE TABLE t2(c0 TEXT UNIQUE) STRICT;
INSERT OR IGNORE INTO t2 VALUES (' a');
```

## Queries

```sql
SELECT x'0102', concat_ws(9007199254740993, '-1', -100, a0.c1), NULL FROM (SELECT 9223372036854775807 AS c0, asinh(1) AS c1, '[1,2,3]' AS c2 FROM t2 AS b28x0) AS a0 GROUP BY x'0102', a0.c1;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    x'0102', '-19007199254740993-10090071992547409930.88137358701954305', NULL
  Amphora:  1 rows
    x'0102', '-19007199254740993-10090071992547409930.881373587019543', NULL
```
