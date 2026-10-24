# cert Estimate: the more restrictive query is estimated at 1367879.4411714424 rows, the query at 1000000

seed 13, state 2388

## Statements

```sql
CREATE TABLE t1(c0 INT, c1 );
```

## Queries

```sql
SELECT NULL FROM t1 AS a0;
SELECT NULL FROM t1 AS a0 WHERE (SELECT count(s964.c1) FROM t1 AS s964 WHERE (a0.c0 = s964.c0));
```

## What differed

```
the more restrictive query is estimated at 1367879.4411714424 rows, the query at 1000000
```
