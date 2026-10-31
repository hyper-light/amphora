# cert Estimate: the more restrictive query is estimated at 1000000 rows, the query at 200000

seed 14, state 18092

## Statements

```sql
CREATE TABLE t0(c0 TEXT, c1 DOUBLE, c2 INTEGER PRIMARY KEY, c3 BOOLEAN);
```

## Queries

```sql
SELECT NULL FROM t0 AS a0 WHERE (a0.c0 LIKE '_');
SELECT NULL FROM t0 AS a0 WHERE ((a0.c0 LIKE '_') AND ((NULL NOT IN (SELECT s845.c3 FROM t0 AS s845 WHERE (7 NOT GLOB 'A%'))) AND ' a'));
```

## What differed

```
the more restrictive query is estimated at 1000000 rows, the query at 200000
```
