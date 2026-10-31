# tlp Wrong: the query's rows aren't its three partitions' (p, NOT p, p IS NULL)

seed 20261024, state 1354

## Statements

```sql
CREATE TABLE t1(c0 DOUBLE COLLATE RTRIM NOT NULL, c1 DECIMAL(10,2), c2 REAL, c3 REAL COLLATE BINARY);
INSERT INTO t1 VALUES ('10', 1000000000000000.0, '-1', x'41');
```

## Queries

```sql
SELECT NULL FROM t1 AS a0;
SELECT NULL FROM t1 AS a0 WHERE (NULL NOT LIKE 'A%');
SELECT NULL FROM t1 AS a0 WHERE (NOT (NULL NOT LIKE 'A%'));
SELECT NULL FROM t1 AS a0 WHERE ((NULL NOT LIKE 'A%') ISNULL);
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
