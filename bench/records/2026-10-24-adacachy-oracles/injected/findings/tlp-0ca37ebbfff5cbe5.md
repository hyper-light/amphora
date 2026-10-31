# tlp Wrong: the query's rows aren't its three partitions' (p, NOT p, p IS NULL)

seed 20261024, state 1036

## Statements

```sql
CREATE TABLE t0(c0 DOUBLE, c1 REAL);
INSERT OR IGNORE INTO t0 VALUES (NULL, 1);
```

## Queries

```sql
SELECT 0.5 FROM t0 AS a0 LEFT JOIN t0 AS a1 ON NULL;
SELECT 0.5 FROM t0 AS a0 LEFT JOIN t0 AS a1 ON NULL WHERE a1.c1;
SELECT 0.5 FROM t0 AS a0 LEFT JOIN t0 AS a1 ON NULL WHERE (NOT a1.c1);
SELECT 0.5 FROM t0 AS a0 LEFT JOIN t0 AS a1 ON NULL WHERE (a1.c1 ISNULL);
```

## What differed

```
the query's rows aren't its three partitions' (p, NOT p, p IS NULL)
  expected: 1 rows
    0.5
  Amphora:  0 rows
```
