# diff Wrong: Amphora's answer isn't SQLite's

seed 12, state 457

## Statements

```sql
CREATE TABLE t0(c0 INTEGER PRIMARY KEY);
INSERT INTO t0 VALUES (-1);
CREATE TABLE t1(c0 TEXT, c1 NUMERIC PRIMARY KEY);
INSERT INTO t1 VALUES (3.0, 'a');
```

## Queries

```sql
SELECT (CASE (a0.c0 COLLATE NOCASE) WHEN ((NOT (x'0102' NOT LIKE 'a%')) AND (a0.c0 IS NOT x'ff')) THEN ('0x10' ISNULL) ELSE zeroblob(a2.c0) END) FROM t0 AS a0 LEFT JOIN t0 AS a1 ON (SELECT sum(s507.c0) FROM t0 AS s507 WHERE ('abc' IS NOT DISTINCT FROM 'a ')), t1 AS a2;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    x'000000'
  Amphora:  1 rows
    x''
```
