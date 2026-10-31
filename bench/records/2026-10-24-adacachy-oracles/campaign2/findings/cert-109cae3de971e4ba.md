# cert Estimate: the more restrictive query is estimated at 900000000000 rows, the query at 360000000000

seed 12, state 970

## Statements

```sql
CREATE TABLE t0(c0 TEXT COLLATE NOCASE, c1 INT, c2 INT UNIQUE);
```

## Queries

```sql
SELECT '' FROM t0 AS a0 FULL JOIN t0 AS a1 ON (100 IS DISTINCT FROM a1.c2) WHERE (ifnull(a1.c0, a0.c2) < a0.c0);
SELECT '' FROM t0 AS a0 JOIN t0 AS a1 ON (100 IS DISTINCT FROM a1.c2) WHERE (ifnull(a1.c0, a0.c2) < a0.c0);
```

## What differed

```
the more restrictive query is estimated at 900000000000 rows, the query at 360000000000
```
