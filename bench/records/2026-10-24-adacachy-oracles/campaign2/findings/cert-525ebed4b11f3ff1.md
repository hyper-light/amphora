# cert Estimate: the more restrictive query is estimated at 200000000000000000 rows, the query at 80000000000000000

seed 12, state 2288

## Statements

```sql
CREATE TABLE t0(c0 BLOB PRIMARY KEY);
CREATE TABLE t1(c0 DECIMAL(10,2) COLLATE BINARY, c1 DECIMAL(10,2), c2 VARCHAR(10));
CREATE TABLE t2(c0 NUMERIC COLLATE BINARY, c1 INT, c2 INT);
```

## Queries

```sql
SELECT ((a1.c0 NOT IN (a1.c0)) AND (1000000000000000.0 NOT GLOB '*')) FROM t2 AS a0 LEFT JOIN t0 AS a1 ON (rtrim('x', a1.c0) NOT LIKE 'a%'), t1 AS a2 WHERE ((a0.c1 COLLATE NOCASE) < log2((CASE WHEN (a1.c0 IS DISTINCT FROM a0.c1) THEN a1.c0 WHEN a1.c0 THEN a2.c1 ELSE a2.c2 END)));
SELECT ((a1.c0 NOT IN (a1.c0)) AND (1000000000000000.0 NOT GLOB '*')) FROM t2 AS a0 LEFT JOIN t0 AS a1 ON (rtrim('x', a1.c0) NOT LIKE 'a%'), t1 AS a2 WHERE (((a0.c1 COLLATE NOCASE) < log2((CASE WHEN (a1.c0 IS DISTINCT FROM a0.c1) THEN a1.c0 WHEN a1.c0 THEN a2.c1 ELSE a2.c2 END))) AND (CAST(a1.c0 AS TEXT) NOT BETWEEN (a2.c0 NOT BETWEEN a1.c0 AND 255) AND a2.c1));
```

## What differed

```
the more restrictive query is estimated at 200000000000000000 rows, the query at 80000000000000000
```
