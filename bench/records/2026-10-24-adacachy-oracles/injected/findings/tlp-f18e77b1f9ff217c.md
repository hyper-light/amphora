# tlp Wrong: the query's rows aren't its three partitions' (p, NOT p, p IS NULL)

seed 20261024, state 942

## Statements

```sql
CREATE TABLE t0(c0 REAL COLLATE BINARY PRIMARY KEY, c1 BOOLEAN COLLATE BINARY);
INSERT INTO t0 VALUES (NULL, -9223372036854775808);
```

## Queries

```sql
SELECT NULL FROM t0 AS a0, t0 AS a1;
SELECT NULL FROM t0 AS a0, t0 AS a1 WHERE ((a1.c1 << a1.c0) || (a0.c0 << a1.c1));
SELECT NULL FROM t0 AS a0, t0 AS a1 WHERE (NOT ((a1.c1 << a1.c0) || (a0.c0 << a1.c1)));
SELECT NULL FROM t0 AS a0, t0 AS a1 WHERE (((a1.c1 << a1.c0) || (a0.c0 << a1.c1)) ISNULL);
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
