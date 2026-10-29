# diff Wrong: Amphora's answer isn't SQLite's

seed 20261028, state 69013

## Statements

```sql
CREATE TABLE t0(c0 BOOLEAN NOT NULL, c1 DECIMAL(10,2) COLLATE RTRIM NOT NULL);
INSERT OR IGNORE INTO t0 VALUES (100, '');
INSERT OR IGNORE INTO t0 VALUES (x'41', 2.0), (3, x'0102');
```

## Queries

```sql
SELECT -100 FROM t0 AS a0 FULL JOIN t0 AS a1 ON ' a' WHERE (NULL IS a0.c0) LIMIT 4 OFFSET 2;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    -100
  Amphora:  0 rows
```
