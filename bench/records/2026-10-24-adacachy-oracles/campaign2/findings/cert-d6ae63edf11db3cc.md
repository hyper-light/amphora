# cert Estimate: the more restrictive query is estimated at 1000000000000000000 rows, the query at 900000000000000000

seed 12, state 3779

## Statements

```sql
CREATE TABLE t1(c0 TEXT, c1 NUMERIC COLLATE NOCASE, c2 NUMERIC, c3 TEXT NOT NULL);
CREATE TABLE t2(c0 BOOLEAN, c1 DOUBLE PRIMARY KEY, c2 TEXT);
```

## Queries

```sql
SELECT a2.c1 FROM t1 AS a0 LEFT JOIN t1 AS a1 ON (-100 IN ((a1.c0 <= -9223372036854775808), (~ a1.c2), json_array_insert(2))), t2 AS a2 WHERE ((CASE '%' WHEN (3.0 IN (a1.c3, 1, a1.c1)) THEN a0.c0 WHEN (a0.c0 NOTNULL) THEN (a1.c0 >> a2.c0) ELSE CAST(a1.c1 AS TEXT) END) <> a0.c1);
SELECT a2.c1 FROM t1 AS a0 LEFT JOIN t1 AS a1 ON (-100 IN ((a1.c0 <= -9223372036854775808), (~ a1.c2), json_array_insert(2))), t2 AS a2 WHERE (((CASE '%' WHEN (3.0 IN (a1.c3, 1, a1.c1)) THEN a0.c0 WHEN (a0.c0 NOTNULL) THEN (a1.c0 >> a2.c0) ELSE CAST(a1.c1 AS TEXT) END) <> a0.c1) AND ((a1.c2 BETWEEN -2.5 AND 1.5) IN ((CASE NULL WHEN ('abc' IN (a2.c0, a2.c0, a1.c3)) THEN a1.c2 ELSE a1.c3 END), (CASE WHEN a1.c2 THEN NULL ELSE a1.c2 END), a1.c2)));
```

## What differed

```
the more restrictive query is estimated at 1000000000000000000 rows, the query at 900000000000000000
```
