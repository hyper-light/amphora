# eet Wrong: an equivalent form gives other rows

seed 20261024, state 962

## Statements

```sql
CREATE TABLE t0(c0 INT, c1 REAL, c2 DOUBLE, c3 DOUBLE COLLATE BINARY UNIQUE);
INSERT INTO t0 VALUES ('abc', 1000000000000000.0, 10, 3);
CREATE TABLE t1(c0 VARCHAR(10) COLLATE NOCASE PRIMARY KEY);
INSERT INTO t1 VALUES (NULL);
```

## Queries

```sql
SELECT a0.c0 FROM t0 AS a0 JOIN t0 AS a1 ON (' ' > json_error_position(a0.c3)), t1 AS a2 WHERE (NOT ((-100 NOT IN (a2.c0, a0.c2, 2)) NOT GLOB 'a%'));
SELECT a0.c0 FROM t0 AS a0 JOIN t0 AS a1 ON (' ' > json_error_position(a0.c3)), t1 AS a2 WHERE (CASE WHEN (a1.c3 IS a0.c3) THEN (NOT (((CASE WHEN (255 IS NOT 3) THEN -100 ELSE -100 END) NOT IN (a2.c0, a0.c2, 2)) NOT GLOB 'a%')) ELSE (NOT (((CASE WHEN (255 IS NOT 3) THEN -100 ELSE -100 END) NOT IN (a2.c0, a0.c2, 2)) NOT GLOB 'a%')) END);
```

## What differed

```
an equivalent form gives other rows
  expected: 1 rows
    'abc'
  Amphora:  0 rows
```
