# tlp Wrong: the query's rows aren't its three partitions' (p, NOT p, p IS NULL)

seed 20261024, state 378

## Statements

```sql
CREATE TABLE t0(c0 DOUBLE, c1 TEXT UNIQUE, c2 VARCHAR(10), c3 REAL);
INSERT INTO t0 VALUES ('x', 0.0, 'é', '');
```

## Queries

```sql
SELECT 1 FROM t0 AS a0, t0 AS a1;
SELECT 1 FROM t0 AS a0, t0 AS a1 WHERE ((NOT a0.c1) / (a1.c2 = a1.c3));
SELECT 1 FROM t0 AS a0, t0 AS a1 WHERE (NOT ((NOT a0.c1) / (a1.c2 = a1.c3)));
SELECT 1 FROM t0 AS a0, t0 AS a1 WHERE (((NOT a0.c1) / (a1.c2 = a1.c3)) ISNULL);
```

## What differed

```
the query's rows aren't its three partitions' (p, NOT p, p IS NULL)
  expected: 1 rows
    1
  Amphora:  2 rows
    1
    1
```
