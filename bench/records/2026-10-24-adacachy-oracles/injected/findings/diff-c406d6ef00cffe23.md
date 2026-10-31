# diff Wrong: Amphora's answer isn't SQLite's

seed 20261024, state 341

## Statements

```sql
CREATE TABLE t1(c0 BLOB, c1 INT PRIMARY KEY);
INSERT OR IGNORE INTO t1 VALUES (0.5, 0.1);
```

## Queries

```sql
SELECT NULL FROM t1 AS a0 WHERE (NOT (NULL NOT GLOB 'abc'));
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 0 rows
  Amphora:  1 rows
    NULL
```
