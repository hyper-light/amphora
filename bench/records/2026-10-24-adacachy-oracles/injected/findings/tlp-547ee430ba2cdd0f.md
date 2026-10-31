# tlp Wrong: the query's rows aren't its three partitions' (p, NOT p, p IS NULL)

seed 20261024, state 1163

## Statements

```sql
CREATE TABLE t0(c0 INTEGER COLLATE BINARY);
INSERT INTO t0 VALUES ('%');
```

## Queries

```sql
SELECT NULL FROM t0 AS a0 LEFT JOIN t0 AS a1 ON NULL;
SELECT NULL FROM t0 AS a0 LEFT JOIN t0 AS a1 ON NULL WHERE a1.c0;
SELECT NULL FROM t0 AS a0 LEFT JOIN t0 AS a1 ON NULL WHERE (NOT a1.c0);
SELECT NULL FROM t0 AS a0 LEFT JOIN t0 AS a1 ON NULL WHERE (a1.c0 ISNULL);
```

## What differed

```
the query's rows aren't its three partitions' (p, NOT p, p IS NULL)
  expected: 1 rows
    NULL
  Amphora:  0 rows
```
