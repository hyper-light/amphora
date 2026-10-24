# tlp Wrong: the query's rows aren't its three partitions' (p, NOT p, p IS NULL)

seed 20261024, state 1008

## Statements

```sql
CREATE TABLE t0(c0 NUMERIC, c1 INT);
INSERT INTO t0 VALUES (1000000000000000.0, 1000000000000000.0);
```

## Queries

```sql
SELECT NULL FROM t0 AS a0;
SELECT NULL FROM t0 AS a0 WHERE (NULL * NULL);
SELECT NULL FROM t0 AS a0 WHERE (NOT (NULL * NULL));
SELECT NULL FROM t0 AS a0 WHERE ((NULL * NULL) ISNULL);
```

## What differed

```
the query's rows aren't its three partitions' (p, NOT p, p IS NULL)
  expected: 1 rows
    NULL
  Amphora:  2 rows
    NULL
    NULL
```
