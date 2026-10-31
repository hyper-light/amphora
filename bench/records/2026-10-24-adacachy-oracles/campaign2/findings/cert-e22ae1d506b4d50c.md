# cert Estimate: the more restrictive query is estimated at 900000000000 rows, the query at 360000000000

seed 12, state 2070

## Statements

```sql
CREATE TABLE t0(c0 );
CREATE TABLE t2(c0 VARCHAR(10) COLLATE BINARY, c1 DOUBLE, c2 INT COLLATE NOCASE, c3 BLOB PRIMARY KEY);
```

## Queries

```sql
SELECT NULL FROM t0 AS a0 LEFT JOIN t2 AS a1 ON (NULL != a1.c2) WHERE ((CASE WHEN a0.c0 THEN NULL ELSE a1.c3 END) BETWEEN NULL AND a0.c0);
SELECT NULL FROM t0 AS a0 JOIN t2 AS a1 ON (NULL != a1.c2) WHERE ((CASE WHEN a0.c0 THEN NULL ELSE a1.c3 END) BETWEEN NULL AND a0.c0);
```

## What differed

```
the more restrictive query is estimated at 900000000000 rows, the query at 360000000000
```
