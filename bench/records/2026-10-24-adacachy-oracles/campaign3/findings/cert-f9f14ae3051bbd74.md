# cert Estimate: the more restrictive query is estimated at 160000000000 rows, the query at 5000000000

seed 13, state 5054

## Statements

```sql
CREATE TABLE t0(c0 VARCHAR(10) PRIMARY KEY, c1 BOOLEAN);
```

## Queries

```sql
SELECT 'é' FROM t0 AS a0 RIGHT JOIN t0 AS a1 ON 2 WHERE (a1.c0 BETWEEN -1 AND (0.0 COLLATE RTRIM));
SELECT 'é' FROM t0 AS a0 JOIN t0 AS a1 ON 2 WHERE (a1.c0 BETWEEN -1 AND (0.0 COLLATE RTRIM));
```

## What differed

```
the more restrictive query is estimated at 160000000000 rows, the query at 5000000000
```
