# cert Estimate: the more restrictive query is estimated at 320000 rows, the query at 10000

seed 12, state 211

## Statements

```sql
CREATE TABLE t0(c0 REAL PRIMARY KEY NOT NULL);
CREATE TABLE t1(c0 VARCHAR(10), c1 TEXT PRIMARY KEY);
```

## Queries

```sql
SELECT NULL FROM t1 AS a0 FULL JOIN t0 AS a1 ON ((a1.c0 > 1000000000000000.0) AND (a0.c1 >= a1.c0)) WHERE (a1.c0 IN (NULL, 1.5));
SELECT NULL FROM t1 AS a0 FULL JOIN t0 AS a1 ON ((a1.c0 > 1000000000000000.0) AND (a0.c1 >= a1.c0)) WHERE ((a1.c0 IN (NULL, 1.5)) AND a0.c1);
```

## What differed

```
the more restrictive query is estimated at 320000 rows, the query at 10000
```
