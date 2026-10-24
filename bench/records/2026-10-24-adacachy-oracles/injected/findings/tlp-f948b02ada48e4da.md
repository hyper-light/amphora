# tlp Wrong: the query's rows aren't its three partitions' (p, NOT p, p IS NULL)

seed 20261024, state 1014

## Statements

```sql
CREATE TABLE t0(c0 DECIMAL(10,2), c1 TEXT, c2 INTEGER);
CREATE TABLE t1(c0 INT PRIMARY KEY, c1 VARCHAR(10), c2 );
INSERT INTO t1 VALUES (1.5, 9007199254740993, '10');
```

## Queries

```sql
SELECT x'0102' FROM t0 AS a0 RIGHT JOIN t1 AS a1 ON NULL;
SELECT x'0102' FROM t0 AS a0 RIGHT JOIN t1 AS a1 ON NULL WHERE a0.c0;
SELECT x'0102' FROM t0 AS a0 RIGHT JOIN t1 AS a1 ON NULL WHERE (NOT a0.c0);
SELECT x'0102' FROM t0 AS a0 RIGHT JOIN t1 AS a1 ON NULL WHERE (a0.c0 ISNULL);
```

## What differed

```
the query's rows aren't its three partitions' (p, NOT p, p IS NULL)
  expected: 1 rows
    x'0102'
  Amphora:  0 rows
```
