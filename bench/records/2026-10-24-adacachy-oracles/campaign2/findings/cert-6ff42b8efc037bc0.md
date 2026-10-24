# cert Estimate: the more restrictive query is estimated at 900000000000000000 rows, the query at 2700000000000

seed 12, state 3620

## Statements

```sql
CREATE TABLE t0(c0 VARCHAR(10), c1 DECIMAL(10,2));
CREATE TABLE t1(c0 DECIMAL(10,2) COLLATE BINARY);
CREATE TABLE t2(c0  PRIMARY KEY, c1 TEXT COLLATE RTRIM, c2 INTEGER COLLATE RTRIM, c3 NUMERIC UNIQUE);
```

## Queries

```sql
SELECT NULL FROM t2 AS a0 RIGHT JOIN t1 AS a1 ON ('A' <> a1.c0) RIGHT JOIN t0 AS a2 ON (a0.c0 IN (char(a1.c0, x'ff', a0.c3), 2, a2.c1)) WHERE a1.c0;
SELECT NULL FROM t2 AS a0 JOIN t1 AS a1 ON ('A' <> a1.c0) RIGHT JOIN t0 AS a2 ON (a0.c0 IN (char(a1.c0, x'ff', a0.c3), 2, a2.c1)) WHERE a1.c0;
```

## What differed

```
the more restrictive query is estimated at 900000000000000000 rows, the query at 2700000000000
```
