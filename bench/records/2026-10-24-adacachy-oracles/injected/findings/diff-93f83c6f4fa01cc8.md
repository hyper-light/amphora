# diff Wrong: Amphora's answer isn't SQLite's

seed 20261024, state 1353

## Statements

```sql
CREATE TABLE t0(c0 INT PRIMARY KEY);
INSERT OR IGNORE INTO t0 VALUES (NULL);
```

## Queries

```sql
SELECT x'41' FROM t0 AS a0 FULL JOIN t0 AS a1 ON NULL WHERE (a1.c0 ISNULL);
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 2 rows
    x'41'
    x'41'
  Amphora:  1 rows
    x'41'
```
