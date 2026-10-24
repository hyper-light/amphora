# cert Estimate: the more restrictive query is estimated at 500000000 rows, the query at 2500000

seed 13, state 4953

## Statements

```sql
CREATE TABLE t0(c0 REAL, c1 , c2 TEXT COLLATE RTRIM PRIMARY KEY);
```

## Queries

```sql
SELECT jsonb_insert((a1.c2 - a1.c2)) FROM t0 AS a0, t0 AS a1 WHERE (((a1.c1 BETWEEN a1.c1 AND NULL) AND (a1.c1 BETWEEN 1.5 AND 'ABC')) AND (ifnull(a0.c0, a1.c1) == (CASE WHEN (a1.c2 IN (a1.c0)) THEN 1000000000000000.0 ELSE 'abc' END)));
SELECT jsonb_insert((a1.c2 - a1.c2)) FROM t0 AS a0, t0 AS a1 WHERE ((((a1.c1 BETWEEN a1.c1 AND NULL) AND (a1.c1 BETWEEN 1.5 AND 'ABC')) AND (ifnull(a0.c0, a1.c1) == (CASE WHEN (a1.c2 IN (a1.c0)) THEN 1000000000000000.0 ELSE 'abc' END))) AND (json_extract('"x"', '$.a') IN (a1.c2, nullif(x'00', NULL), ('0' NOT IN (0)))));
```

## What differed

```
the more restrictive query is estimated at 500000000 rows, the query at 2500000
```
