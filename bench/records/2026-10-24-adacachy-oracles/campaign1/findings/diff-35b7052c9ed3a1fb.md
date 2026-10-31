# diff Wrong: Amphora's answer isn't SQLite's

seed 11, state 480

## Statements

```sql
CREATE TABLE t0(c0 BLOB, c1 DOUBLE, c2 NUMERIC UNIQUE, c3 TEXT COLLATE RTRIM);
INSERT INTO t0 VALUES (1.5, 7, 7, x'0102');
CREATE TABLE t1(c0 INT COLLATE RTRIM PRIMARY KEY, c1 BLOB, c2 BOOLEAN COLLATE BINARY);
INSERT INTO t1 VALUES (x'ff', 7, 7);
```

## Queries

```sql
SELECT count(*) FROM t0 AS a0, t0 AS a1, t1 AS a2 WHERE ((NOT (a2.c0 GLOB '*')) OR ((CASE WHEN ('-1' IN (a0.c0, a1.c0, a1.c1)) THEN a2.c0 WHEN (1e308 NOT GLOB '%a') THEN a1.c2 ELSE a2.c0 END) == a2.c0));
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    0
  Amphora:  1 rows
    1
```
