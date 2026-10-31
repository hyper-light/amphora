# cert Estimate: the more restrictive query is estimated at 900000000000 rows, the query at 1800000

seed 12, state 1234

## Statements

```sql
CREATE TABLE t1(c0 INT COLLATE NOCASE, c1 BOOLEAN, c2 INT COLLATE NOCASE);
```

## Queries

```sql
SELECT (- (a1.c0 != 'é')) FROM t1 AS a0, t1 AS a1 WHERE ((('a' IN (a1.c1, a0.c0)) AND (a0.c0 IN (a0.c0, a1.c2))) AND (a0.c0 IS NOT tanh(a1.c1)));
SELECT (- (a1.c0 != 'é')) FROM t1 AS a0, t1 AS a1 WHERE (((('a' IN (a1.c1, a0.c0)) AND (a0.c0 IN (a0.c0, a1.c2))) AND (a0.c0 IS NOT tanh(a1.c1))) AND ((-100 IN ('é', '1')) OR ((a1.c0 IS 1.5) OR (a1.c2 NOT IN (NULL, '1.5')))));
```

## What differed

```
the more restrictive query is estimated at 900000000000 rows, the query at 1800000
```
