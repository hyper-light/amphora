# cert Estimate: the more restrictive query is estimated at 400000000000 rows, the query at 5000000000

seed 12, state 2062

## Statements

```sql
CREATE TABLE t1(c0 VARCHAR(10) COLLATE NOCASE);
```

## Queries

```sql
SELECT (~ (a1.c0 - a0.c0)) FROM t1 AS a0, t1 AS a1 WHERE (a0.c0 BETWEEN ((-100 = a1.c0) OR (NOT (a0.c0 LIKE '%'))) AND like(a0.c0, lower(a0.c0), jsonb_insert(a1.c0, a0.c0, 100)));
SELECT (~ (a1.c0 - a0.c0)) FROM t1 AS a0, t1 AS a1 WHERE ((a0.c0 BETWEEN ((-100 = a1.c0) OR (NOT (a0.c0 LIKE '%'))) AND like(a0.c0, lower(a0.c0), jsonb_insert(a1.c0, a0.c0, 100))) AND (CAST(a1.c0 AS NUMERIC) >= ' '));
```

## What differed

```
the more restrictive query is estimated at 400000000000 rows, the query at 5000000000
```
