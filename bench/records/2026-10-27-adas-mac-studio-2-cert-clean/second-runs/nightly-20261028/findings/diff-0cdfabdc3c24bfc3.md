# diff Wrong: Amphora's answer isn't SQLite's

seed 20261028, state 20680

## Statements

```sql
CREATE TABLE t0(c0  PRIMARY KEY, c1 TEXT, c2 INTEGER);
CREATE TABLE t1(c0 DOUBLE, c1 INTEGER PRIMARY KEY);
INSERT OR IGNORE INTO t1 VALUES (0.0, 0.0);
INSERT OR IGNORE INTO t1 VALUES ('é', 3), (2.0, 7);
```

## Queries

```sql
SELECT 7 FROM t0 AS a0 RIGHT JOIN t1 AS a2 ON NULL LIMIT 1 OFFSET 2;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    7
  Amphora:  0 rows
```
