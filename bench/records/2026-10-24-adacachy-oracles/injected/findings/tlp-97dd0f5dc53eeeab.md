# tlp Wrong: the query's rows aren't its three partitions' (p, NOT p, p IS NULL)

seed 20261024, state 690

## Statements

```sql
CREATE TABLE t1(c0 INT, c1 REAL, c2 DECIMAL(10,2) PRIMARY KEY NOT NULL);
INSERT OR IGNORE INTO t1 VALUES (7, 'a', -100);
```

## Queries

```sql
SELECT 2.0 FROM t1 AS a0;
SELECT 2.0 FROM t1 AS a0 WHERE (NULL & 1.5);
SELECT 2.0 FROM t1 AS a0 WHERE (NOT (NULL & 1.5));
SELECT 2.0 FROM t1 AS a0 WHERE ((NULL & 1.5) ISNULL);
```

## What differed

```
the query's rows aren't its three partitions' (p, NOT p, p IS NULL)
  expected: 1 rows
    2.0
  Amphora:  2 rows
    2.0
    2.0
```
