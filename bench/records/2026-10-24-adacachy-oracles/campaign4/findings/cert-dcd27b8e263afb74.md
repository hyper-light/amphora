# cert Estimate: the more restrictive query is estimated at 5000 rows, the query at 2000

seed 14, state 19789

## Statements

```sql
CREATE TABLE t1(c0 BLOB, c1 NUMERIC COLLATE NOCASE PRIMARY KEY);
```

## Queries

```sql
SELECT NULL FROM t1 AS a0 WHERE (a0.c1 BETWEEN a0.c1 AND '[1,2,3]');
SELECT NULL FROM t1 AS a0 WHERE ((a0.c1 BETWEEN a0.c1 AND '[1,2,3]') AND a0.c0);
```

## What differed

```
the more restrictive query is estimated at 5000 rows, the query at 2000
```
