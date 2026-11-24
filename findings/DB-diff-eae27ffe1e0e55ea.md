# diff Wrong: Amphora's answer isn't SQLite's

seed 8005, state 17011

## Statements

```sql
CREATE TABLE t0(c0 NUMERIC COLLATE RTRIM);
INSERT INTO t0 VALUES (10), ('a ');
```

## Queries

```sql
SELECT sum((CAST((a2.c0 BETWEEN a0.c0 AND a2.c0) AS INTEGER) % 100)) OVER (ORDER BY a0.rowid, a1.rowid DESC, a2.rowid DESC GROUPS BETWEEN 1 FOLLOWING AND UNBOUNDED FOLLOWING) FROM t0 AS a0 JOIN t0 AS a1 ON (((a1.c0, a0.c0) BETWEEN (-2.5, 100) AND (10, '1e3')) AND (NOT (a0.c0 == -9223372036854775808))), t0 AS a2;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 2 rows
    1
    NULL
  Amphora:  4 rows
    2
    1
    0
    NULL
```
