# cert Estimate: the more restrictive query is estimated at 1367879.4411714424 rows, the query at 1000000

seed 14, state 16314

## Statements

```sql
CREATE TABLE t0(c0 REAL NOT NULL, c1  COLLATE NOCASE, c2 DECIMAL(10,2), c3  COLLATE BINARY);
```

## Queries

```sql
SELECT NULL FROM t0 AS a0;
SELECT NULL FROM t0 AS a0 WHERE (SELECT count(s513.c1) FROM t0 AS s513 WHERE (a0.c1 = s513.c1));
```

## What differed

```
the more restrictive query is estimated at 1367879.4411714424 rows, the query at 1000000
```
