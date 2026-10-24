# tlp Wrong: the query's rows aren't its three partitions' (p, NOT p, p IS NULL)

seed 20261024, state 663

## Statements

```sql
CREATE TABLE t0(c0 VARCHAR(10) PRIMARY KEY, c1 TEXT COLLATE NOCASE, c2 VARCHAR(10) COLLATE RTRIM, c3 BOOLEAN);
INSERT INTO t0 VALUES (1e308, NULL, 0, -100);
```

## Queries

```sql
SELECT NULL, NULL, NULL FROM t0 AS a0 LEFT JOIN t0 AS a1 ON NULL GROUP BY NULL, a1.c0;
SELECT NULL, NULL, NULL FROM t0 AS a0 LEFT JOIN t0 AS a1 ON NULL GROUP BY NULL, a1.c0 HAVING a1.c0;
SELECT NULL, NULL, NULL FROM t0 AS a0 LEFT JOIN t0 AS a1 ON NULL GROUP BY NULL, a1.c0 HAVING (NOT a1.c0);
SELECT NULL, NULL, NULL FROM t0 AS a0 LEFT JOIN t0 AS a1 ON NULL GROUP BY NULL, a1.c0 HAVING (a1.c0 ISNULL);
```

## What differed

```
the query's rows aren't its three partitions' (p, NOT p, p IS NULL)
  expected: 1 rows
    NULL, NULL, NULL
  Amphora:  0 rows
```
