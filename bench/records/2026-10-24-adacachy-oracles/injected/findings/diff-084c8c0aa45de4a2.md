# diff Wrong: Amphora's answer isn't SQLite's

seed 20261024, state 821

## Statements

```sql
CREATE TABLE t0(c0 BOOLEAN, c1 INT, c2 REAL);
INSERT INTO t0 VALUES (-1, NULL, '10');
CREATE TABLE t1(c0 DOUBLE COLLATE NOCASE UNIQUE NOT NULL, c1 DOUBLE);
```

## Queries

```sql
SELECT x'' FROM t0 AS a0 LEFT JOIN t1 AS a1 ON '{"a":null}' WHERE (a1.c1 ISNULL);
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    x''
  Amphora:  0 rows
```
