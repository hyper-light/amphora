# cert Estimate: the more restrictive query is estimated at 1000000000000 rows, the query at 100000000000

seed 12, state 42

## Statements

```sql
CREATE TABLE t0(c0 INTEGER, c1 TEXT UNIQUE, c2 NUMERIC);
CREATE TABLE t2(c0 REAL, c1 REAL);
```

## Queries

```sql
SELECT NULL FROM t2 AS a0 JOIN t0 AS a1 ON ((a0.c0 BETWEEN 2.0 AND a1.c0) = a0.c1);
SELECT NULL FROM t2 AS a0 JOIN t0 AS a1 ON ((a0.c0 BETWEEN 2.0 AND a1.c0) = a0.c1) WHERE (NOT (0.0 BETWEEN NULL AND a1.c0));
```

## What differed

```
the more restrictive query is estimated at 1000000000000 rows, the query at 100000000000
```
