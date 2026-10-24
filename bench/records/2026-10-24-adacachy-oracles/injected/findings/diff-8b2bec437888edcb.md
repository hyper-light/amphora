# diff Wrong: Amphora's answer isn't SQLite's

seed 20261024, state 1646

## Statements

```sql
CREATE TABLE t1(c0 NUMERIC, c1 REAL, c2 INT COLLATE BINARY);
INSERT OR IGNORE INTO t1 VALUES (0.5, 'true', x'');
```

## Queries

```sql
SELECT NULL FROM t1 AS a0 JOIN t1 AS a1 ON (NOT (NULL AND a1.c1));
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    NULL
  Amphora:  0 rows
```
