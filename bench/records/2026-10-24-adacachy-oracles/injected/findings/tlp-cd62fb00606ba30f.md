# tlp Wrong: the query's rows aren't its three partitions' (p, NOT p, p IS NULL)

seed 20261024, state 851

## Statements

```sql
CREATE TABLE t1(c0 DECIMAL(10,2), c1 BOOLEAN UNIQUE);
CREATE TABLE t2(c0 DECIMAL(10,2) PRIMARY KEY, c1 BLOB COLLATE BINARY, c2 INTEGER);
INSERT INTO t2 VALUES (1.5, 1000000000000000.0, '1e3');
```

## Queries

```sql
SELECT (CASE WHEN (a0.c0 <> 0.5) THEN (a2.c1 % a1.c1) WHEN (x'ff' IS NOT DISTINCT FROM a2.c1) THEN a2.c1 ELSE (- a0.c1) END) FROM t2 AS a0, t2 AS a1 LEFT JOIN t1 AS a2 ON ((a0.c1 <> 0.0) < ' ');
SELECT (CASE WHEN (a0.c0 <> 0.5) THEN (a2.c1 % a1.c1) WHEN (x'ff' IS NOT DISTINCT FROM a2.c1) THEN a2.c1 ELSE (- a0.c1) END) FROM t2 AS a0, t2 AS a1 LEFT JOIN t1 AS a2 ON ((a0.c1 <> 0.0) < ' ') WHERE ((acos(a0.c1) / json_extract('[1,2,3]', '$[0]')) NOT GLOB 'a%');
SELECT (CASE WHEN (a0.c0 <> 0.5) THEN (a2.c1 % a1.c1) WHEN (x'ff' IS NOT DISTINCT FROM a2.c1) THEN a2.c1 ELSE (- a0.c1) END) FROM t2 AS a0, t2 AS a1 LEFT JOIN t1 AS a2 ON ((a0.c1 <> 0.0) < ' ') WHERE (NOT ((acos(a0.c1) / json_extract('[1,2,3]', '$[0]')) NOT GLOB 'a%'));
SELECT (CASE WHEN (a0.c0 <> 0.5) THEN (a2.c1 % a1.c1) WHEN (x'ff' IS NOT DISTINCT FROM a2.c1) THEN a2.c1 ELSE (- a0.c1) END) FROM t2 AS a0, t2 AS a1 LEFT JOIN t1 AS a2 ON ((a0.c1 <> 0.0) < ' ') WHERE (((acos(a0.c1) / json_extract('[1,2,3]', '$[0]')) NOT GLOB 'a%') ISNULL);
```

## What differed

```
the query's rows aren't its three partitions' (p, NOT p, p IS NULL)
  expected: 1 rows
    NULL
  Amphora:  2 rows
    NULL
    NULL
```
