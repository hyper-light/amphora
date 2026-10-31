# cert Estimate: the more restrictive query is estimated at 160000000000 rows, the query at 5000000000

seed 20261024, state 1553

## Statements

```sql
CREATE TABLE t0(c0 , c1 INTEGER PRIMARY KEY);
CREATE TABLE t1(c0 REAL, c1 DOUBLE COLLATE RTRIM PRIMARY KEY NOT NULL, c2 INT);
```

## Queries

```sql
SELECT NULL FROM t0 AS a0 RIGHT JOIN t1 AS a1 ON NULL WHERE (a1.c1 BETWEEN (CASE WHEN NULL THEN a0.c1 ELSE a1.c2 END) AND '-1');
SELECT NULL FROM t0 AS a0 JOIN t1 AS a1 ON NULL WHERE (a1.c1 BETWEEN (CASE WHEN NULL THEN a0.c1 ELSE a1.c2 END) AND '-1');
```

## What differed

```
the more restrictive query is estimated at 160000000000 rows, the query at 5000000000
```
