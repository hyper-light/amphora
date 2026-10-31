# cert Estimate: the more restrictive query is estimated at 400000000000 rows, the query at 160000000000.00003

seed 12, state 3503

## Statements

```sql
CREATE TABLE t1(c0  COLLATE BINARY, c1 REAL, c2 DECIMAL(10,2) PRIMARY KEY);
```

## Queries

```sql
SELECT (CASE WHEN ('x' NOTNULL) THEN a1.c0 WHEN (7 == a1.c2) THEN (a0.c1 COLLATE RTRIM) END) FROM t1 AS a0, t1 AS a1 WHERE ((sqrt(a1.c0) || (+ a0.c0)) BETWEEN (a0.c2 IS NOT '%') AND a0.c2);
SELECT (CASE WHEN ('x' NOTNULL) THEN a1.c0 WHEN (7 == a1.c2) THEN (a0.c1 COLLATE RTRIM) END) FROM t1 AS a0, t1 AS a1 WHERE (((sqrt(a1.c0) || (+ a0.c0)) BETWEEN (a0.c2 IS NOT '%') AND a0.c2) AND ((a1.c2 >= a1.c2) > a0.c2));
```

## What differed

```
the more restrictive query is estimated at 400000000000 rows, the query at 160000000000.00003
```
