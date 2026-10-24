# tlp Wrong: the query's rows aren't its three partitions' (p, NOT p, p IS NULL)

seed 20261024, state 499

## Statements

```sql
CREATE TABLE t2(c0 REAL);
INSERT INTO t2 VALUES (NULL);
```

## Queries

```sql
SELECT NULL FROM t2 AS a0 LEFT JOIN t2 AS a1 ON NULL;
SELECT NULL FROM t2 AS a0 LEFT JOIN t2 AS a1 ON NULL WHERE a1.c0;
SELECT NULL FROM t2 AS a0 LEFT JOIN t2 AS a1 ON NULL WHERE (NOT a1.c0);
SELECT NULL FROM t2 AS a0 LEFT JOIN t2 AS a1 ON NULL WHERE (a1.c0 ISNULL);
```

## What differed

```
the query's rows aren't its three partitions' (p, NOT p, p IS NULL)
  expected: 1 rows
    NULL
  Amphora:  0 rows
```
