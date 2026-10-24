# tlp Wrong: the query's rows aren't its three partitions' (p, NOT p, p IS NULL)

seed 20261024, state 546

## Statements

```sql
CREATE TABLE t0(c0 BOOLEAN COLLATE RTRIM UNIQUE, c1 INT, c2 NUMERIC);
INSERT OR IGNORE INTO t0 VALUES ('1e3', 1000000000000000.0, x'0102');
```

## Queries

```sql
SELECT 'abc' FROM t0 AS a0 LEFT JOIN t0 AS a1 ON 0.0;
SELECT 'abc' FROM t0 AS a0 LEFT JOIN t0 AS a1 ON 0.0 WHERE a1.c0;
SELECT 'abc' FROM t0 AS a0 LEFT JOIN t0 AS a1 ON 0.0 WHERE (NOT a1.c0);
SELECT 'abc' FROM t0 AS a0 LEFT JOIN t0 AS a1 ON 0.0 WHERE (a1.c0 ISNULL);
```

## What differed

```
the query's rows aren't its three partitions' (p, NOT p, p IS NULL)
  expected: 1 rows
    'abc'
  Amphora:  0 rows
```
