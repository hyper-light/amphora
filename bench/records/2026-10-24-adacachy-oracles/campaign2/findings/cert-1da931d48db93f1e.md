# cert Estimate: the more restrictive query is estimated at 1000000000000 rows, the query at 400000000000

seed 12, state 373

## Statements

```sql
CREATE TABLE t0(c0 BLOB COLLATE RTRIM PRIMARY KEY NOT NULL);
CREATE TABLE t1(c0 BOOLEAN, c1 DECIMAL(10,2) COLLATE BINARY);
```

## Queries

```sql
SELECT (a0.c0 AND (a1.c0 >= a0.c1)) FROM t1 AS a0, t1 AS a1 WHERE ((SELECT count(s101.c0) FROM t0 AS s101 WHERE (NOT (a0.c0 IS NOT DISTINCT FROM '0'))) >= ((a0.c0 & '%') + (a1.c1 ISNULL)));
SELECT (a0.c0 AND (a1.c0 >= a0.c1)) FROM t1 AS a0, t1 AS a1 WHERE (((SELECT count(s101.c0) FROM t0 AS s101 WHERE (NOT (a0.c0 IS NOT DISTINCT FROM '0'))) >= ((a0.c0 & '%') + (a1.c1 ISNULL))) AND (NOT (NOT (a1.c1 BETWEEN a1.c0 AND -9223372036854775808))));
```

## What differed

```
the more restrictive query is estimated at 1000000000000 rows, the query at 400000000000
```
