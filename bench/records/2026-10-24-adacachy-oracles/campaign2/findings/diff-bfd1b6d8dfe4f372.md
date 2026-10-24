# diff Wrong: Amphora's answer isn't SQLite's

seed 12, state 3015

## Statements

```sql
CREATE TABLE t1(c0  PRIMARY KEY, c1 NUMERIC);
CREATE TABLE t2(c0 DOUBLE COLLATE BINARY, c1 DOUBLE, c2 INT);
INSERT OR IGNORE INTO t2 VALUES (255, 10, '');
```

## Queries

```sql
SELECT replace(1, a2.c2, a1.c0), (~ a0.c0), count(*) FROM t1 AS a0, t2 AS a1 RIGHT JOIN t2 AS a2 ON (glob(3, NULL) < a0.c0) GROUP BY replace(1, a2.c2, a1.c0), (~ a0.c0);
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    '1', NULL, 1
  Amphora:  1 rows
    NULL, NULL, 1
```
