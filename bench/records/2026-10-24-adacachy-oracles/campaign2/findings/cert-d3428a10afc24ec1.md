# cert Estimate: the more restrictive query is estimated at 1000000000000000000 rows, the query at 400000000000000000

seed 12, state 1832

## Statements

```sql
CREATE TABLE t0(c0 DOUBLE PRIMARY KEY, c1 BOOLEAN COLLATE BINARY UNIQUE, c2 );
CREATE TABLE t1(c0 NUMERIC COLLATE RTRIM NOT NULL, c1 DOUBLE UNIQUE, c2 BLOB, c3 VARCHAR(10) COLLATE RTRIM);
```

## Queries

```sql
SELECT (a1.c1 >= 10) FROM t0 AS a0, t0 AS a1 JOIN t1 AS a2 ON ((NOT a2.c3) < (CASE WHEN (NOT (a2.c3 != a0.c1)) THEN x'' WHEN (NOT (a0.c1 BETWEEN a0.c1 AND a1.c0)) THEN 'x' ELSE a0.c1 END));
SELECT (a1.c1 >= 10) FROM t0 AS a0, t0 AS a1 JOIN t1 AS a2 ON ((NOT a2.c3) < (CASE WHEN (NOT (a2.c3 != a0.c1)) THEN x'' WHEN (NOT (a0.c1 BETWEEN a0.c1 AND a1.c0)) THEN 'x' ELSE a0.c1 END)) WHERE ((CASE WHEN (NULL >= a1.c2) THEN a0.c0 ELSE a0.c1 END) IN (replace(a2.c1, a2.c3, a2.c0), json_extract('"x"', '$.b[1]'), (CASE WHEN (NOT ('1' NOT BETWEEN a1.c2 AND a1.c0)) THEN a1.c1 WHEN (a2.c3 NOT BETWEEN 'A' AND a0.c1) THEN a1.c1 ELSE a0.c2 END)));
```

## What differed

```
the more restrictive query is estimated at 1000000000000000000 rows, the query at 400000000000000000
```
