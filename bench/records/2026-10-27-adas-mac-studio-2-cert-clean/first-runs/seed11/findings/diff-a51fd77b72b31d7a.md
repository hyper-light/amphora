# diff Wrong: Amphora's answer isn't SQLite's

seed 11, state 33916

## Statements

```sql
CREATE TABLE t0(c0 VARCHAR(10), c1 NUMERIC COLLATE RTRIM, c2 NUMERIC);
INSERT OR IGNORE INTO t0 VALUES (2.0, NULL, -2.5);
CREATE TABLE t1(c0 DOUBLE PRIMARY KEY, c1 DOUBLE);
INSERT OR IGNORE INTO t1 VALUES ('é', 1), (9223372036854775807, x'41');
CREATE TABLE t2(c0 TEXT, c1 VARCHAR(10));
INSERT INTO t2 VALUES (-2.5, x'00');
```

## Queries

```sql
SELECT 'true' FROM t1 AS a0 RIGHT JOIN t2 AS a1 ON ((a1.c0 || a1.c1) IN (SELECT s32.c2 FROM t0 AS s32));
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 2 rows
    'true'
    'true'
  Amphora:  1 rows
    'true'
```
