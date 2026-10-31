# diff Wrong: Amphora's answer isn't SQLite's

seed 12, state 2644

## Statements

```sql
CREATE TABLE t0(c0 DOUBLE COLLATE BINARY, c1 VARCHAR(10) COLLATE NOCASE PRIMARY KEY);
INSERT INTO t0 VALUES ('-1', 2.0);
CREATE TABLE t1(c0 DOUBLE COLLATE NOCASE, c1 REAL);
INSERT OR IGNORE INTO t1 VALUES (10, -2.5);
```

## Queries

```sql
SELECT jsonb_patch(ceil(a2.c0), ('0' % a1.c0)) FROM t0 AS a0 RIGHT JOIN t1 AS a1 ON ((1 IS DISTINCT FROM a0.c1) NOT GLOB '*'), t0 AS a2;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    x'c503302e30'
  Amphora:  1 rows
    x'35302e30'
```
