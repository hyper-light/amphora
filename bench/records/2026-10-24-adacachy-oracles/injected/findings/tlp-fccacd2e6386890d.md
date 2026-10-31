# tlp Wrong: the query's rows aren't its three partitions' (p, NOT p, p IS NULL)

seed 20261024, state 772

## Statements

```sql
CREATE TABLE t0(c0 DOUBLE PRIMARY KEY, c1 REAL COLLATE NOCASE UNIQUE);
INSERT INTO t0 VALUES (10, '');
```

## Queries

```sql
SELECT NULL FROM t0 AS a0;
SELECT NULL FROM t0 AS a0 WHERE (NULL AND a0.c1);
SELECT NULL FROM t0 AS a0 WHERE (NOT (NULL AND a0.c1));
SELECT NULL FROM t0 AS a0 WHERE ((NULL AND a0.c1) ISNULL);
```

## What differed

```
the query's rows aren't its three partitions' (p, NOT p, p IS NULL)
  expected: 1 rows
    NULL
  Amphora:  0 rows
```
