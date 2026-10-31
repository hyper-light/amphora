# diff Wrong: Amphora's answer isn't SQLite's

seed 12, state 57490

## Statements

```sql
CREATE TABLE t0(c0 BLOB COLLATE NOCASE PRIMARY KEY);
INSERT OR IGNORE INTO t0 VALUES (-1e-308);
```

## Queries

```sql
SELECT 3 FROM t0 AS a0 LEFT JOIN t0 AS a2 ON (a2.c0 LIKE 'a%');
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 0 rows
  Amphora:  1 rows
    3
```
