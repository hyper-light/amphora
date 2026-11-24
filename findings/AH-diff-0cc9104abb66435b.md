# diff Wrong: Amphora's answer isn't SQLite's

seed 27010, state 2358

## Statements

```sql
CREATE TABLE t0(c0 NUMERIC UNIQUE);
INSERT INTO t0 VALUES (100);
CREATE INDEX i0 ON t0(c0 COLLATE NOCASE DESC);
```

## Queries

```sql
SELECT 0 FROM t0 AS a0 LEFT JOIN t0 AS a1 ON (a1.c0 LIKE 'a%');
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 0 rows
  Amphora:  1 rows
    0
```
