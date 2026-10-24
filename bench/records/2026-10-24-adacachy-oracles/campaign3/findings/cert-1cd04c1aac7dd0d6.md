# cert Estimate: the more restrictive query is estimated at 160000000000 rows, the query at 5000000000

seed 13, state 10438

## Statements

```sql
CREATE TABLE t0(c0 INTEGER);
```

## Queries

```sql
SELECT '1e3' FROM t0 AS a0 FULL JOIN t0 AS a1 ON 0.1 WHERE (a0.c0 BETWEEN 7 AND (a0.c0 IN (NULL, a1.c0, '')));
SELECT '1e3' FROM t0 AS a0 JOIN t0 AS a1 ON 0.1 WHERE (a0.c0 BETWEEN 7 AND (a0.c0 IN (NULL, a1.c0, '')));
```

## What differed

```
the more restrictive query is estimated at 160000000000 rows, the query at 5000000000
```
