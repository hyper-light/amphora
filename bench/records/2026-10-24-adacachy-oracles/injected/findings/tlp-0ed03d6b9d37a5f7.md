# tlp Wrong: the query's rows aren't its three partitions' (p, NOT p, p IS NULL)

seed 20261024, state 1346

## Statements

```sql
CREATE TABLE t0(c0 VARCHAR(10) COLLATE RTRIM, c1 DECIMAL(10,2), c2 VARCHAR(10), c3 NUMERIC);
INSERT INTO t0 VALUES (' a', -2.5, x'0102', 255);
CREATE TABLE t1(c0 INT);
```

## Queries

```sql
SELECT NULL, NULL FROM t1 AS a0 RIGHT JOIN t0 AS a1 ON NULL GROUP BY a0.c0;
SELECT NULL, NULL FROM t1 AS a0 RIGHT JOIN t0 AS a1 ON NULL GROUP BY a0.c0 HAVING a0.c0;
SELECT NULL, NULL FROM t1 AS a0 RIGHT JOIN t0 AS a1 ON NULL GROUP BY a0.c0 HAVING (NOT a0.c0);
SELECT NULL, NULL FROM t1 AS a0 RIGHT JOIN t0 AS a1 ON NULL GROUP BY a0.c0 HAVING (a0.c0 ISNULL);
```

## What differed

```
the query's rows aren't its three partitions' (p, NOT p, p IS NULL)
  expected: 1 rows
    NULL, NULL
  Amphora:  0 rows
```
