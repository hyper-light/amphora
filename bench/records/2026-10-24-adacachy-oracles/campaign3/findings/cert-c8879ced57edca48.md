# cert Estimate: the more restrictive query is estimated at 25600000000.000008 rows, the query at 5000000000

seed 13, state 4978

## Statements

```sql
CREATE TABLE t0(c0 TEXT NOT NULL, c1 VARCHAR(10));
CREATE TABLE t1(c0 INT, c1 BOOLEAN PRIMARY KEY);
```

## Queries

```sql
SELECT CAST(a1.c0 AS INTEGER) FROM t0 AS a0, t1 AS a1 WHERE (a1.c1 BETWEEN ((a1.c1 / a1.c0) / a0.c1) AND (- cosh(3)));
SELECT CAST(a1.c0 AS INTEGER) FROM t0 AS a0, t1 AS a1 WHERE ((a1.c1 BETWEEN ((a1.c1 / a1.c0) / a0.c1) AND (- cosh(3))) AND (a0.c0 < (100 IS NOT a0.c0)));
```

## What differed

```
the more restrictive query is estimated at 25600000000.000008 rows, the query at 5000000000
```
