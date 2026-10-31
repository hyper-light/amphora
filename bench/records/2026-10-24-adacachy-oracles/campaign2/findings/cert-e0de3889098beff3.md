# cert Estimate: the more restrictive query is estimated at 900000000000 rows, the query at 100000000000

seed 12, state 2218

## Statements

```sql
CREATE TABLE t2(c0  COLLATE RTRIM NOT NULL, c1 BOOLEAN);
```

## Queries

```sql
SELECT datetime((a1.c0 BETWEEN '1.5' AND a0.c1)) FROM t2 AS a0, t2 AS a1 WHERE (((NULL BETWEEN a1.c1 AND a0.c0) = (-1e-308 || a0.c1)) AND (a0.c0 ISNULL));
SELECT datetime((a1.c0 BETWEEN '1.5' AND a0.c1)) FROM t2 AS a0, t2 AS a1 WHERE ((((NULL BETWEEN a1.c1 AND a0.c0) = (-1e-308 || a0.c1)) AND (a0.c0 ISNULL)) AND (CAST(255 AS NUMERIC) IS DISTINCT FROM a1.c0));
```

## What differed

```
the more restrictive query is estimated at 900000000000 rows, the query at 100000000000
```
