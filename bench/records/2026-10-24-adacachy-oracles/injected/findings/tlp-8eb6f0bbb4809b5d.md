# tlp Wrong: the query's rows aren't its three partitions' (p, NOT p, p IS NULL)

seed 20261024, state 919

## Statements

```sql
CREATE TABLE t0(c0 DOUBLE PRIMARY KEY, c1 TEXT UNIQUE);
INSERT INTO t0 VALUES ('a', 2);
```

## Queries

```sql
SELECT 3.0 FROM t0 AS a0 LEFT JOIN t0 AS a1 ON NULL;
SELECT 3.0 FROM t0 AS a0 LEFT JOIN t0 AS a1 ON NULL WHERE a1.c0;
SELECT 3.0 FROM t0 AS a0 LEFT JOIN t0 AS a1 ON NULL WHERE (NOT a1.c0);
SELECT 3.0 FROM t0 AS a0 LEFT JOIN t0 AS a1 ON NULL WHERE (a1.c0 ISNULL);
```

## What differed

```
the query's rows aren't its three partitions' (p, NOT p, p IS NULL)
  expected: 1 rows
    3.0
  Amphora:  0 rows
```
