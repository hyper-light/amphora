# cert Estimate: the more restrictive query is estimated at 1000000000000000000 rows, the query at 100000000000000000

seed 12, state 3461

## Statements

```sql
CREATE TABLE t0(c0 INTEGER COLLATE BINARY PRIMARY KEY);
```

## Queries

```sql
SELECT NULL FROM t0 AS a0 JOIN t0 AS a1 ON ((a0.c0 < NULL) = date(a0.c0, 'é', a1.c0)) LEFT JOIN t0 AS a2 ON 0.5;
SELECT NULL FROM t0 AS a0 JOIN t0 AS a1 ON ((a0.c0 < NULL) = date(a0.c0, 'é', a1.c0)) JOIN t0 AS a2 ON 0.5;
```

## What differed

```
the more restrictive query is estimated at 1000000000000000000 rows, the query at 100000000000000000
```
