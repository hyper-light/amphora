# diff Wrong: Amphora's answer isn't SQLite's

seed 8002, state 19228

## Statements

```sql
CREATE TABLE t1(c0 REAL COLLATE BINARY PRIMARY KEY) WITHOUT ROWID;
CREATE TABLE t2(c0 VARCHAR(10));
INSERT OR REPLACE INTO t1 VALUES (10);
```

## Queries

```sql
SELECT '1', NULL, NULL FROM (SELECT b19x0.c0 AS c0, NULL AS c1 FROM t2 AS b19x0) AS a0 NATURAL FULL JOIN t1 AS a1 WHERE (c0 BETWEEN 3.0 AND 'ABC') GROUP BY '1', NULL;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    '1', NULL, NULL
  Amphora:  0 rows
```
