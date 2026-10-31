# cert Estimate: the more restrictive query is estimated at 160000000000 rows, the query at 5000000000

seed 14, state 19098

## Statements

```sql
CREATE TABLE t1(c0 VARCHAR(10));
```

## Queries

```sql
SELECT 1e308 FROM t1 AS a0 FULL JOIN t1 AS a1 ON 0.5 WHERE (a0.c0 BETWEEN jsonb_insert(1000000000000000.0, a1.c0, a0.c0) AND 1);
SELECT 1e308 FROM t1 AS a0 JOIN t1 AS a1 ON 0.5 WHERE (a0.c0 BETWEEN jsonb_insert(1000000000000000.0, a1.c0, a0.c0) AND 1);
```

## What differed

```
the more restrictive query is estimated at 160000000000 rows, the query at 5000000000
```
