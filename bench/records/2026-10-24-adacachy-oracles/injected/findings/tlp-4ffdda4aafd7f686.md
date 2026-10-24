# tlp Wrong: the query's rows aren't the UNION of its partitions'

seed 20261024, state 587

## Statements

```sql
CREATE TABLE t0(c0 INT PRIMARY KEY, c1 INTEGER, c2 DECIMAL(10,2) COLLATE RTRIM, c3 INT COLLATE RTRIM);
INSERT OR IGNORE INTO t0 VALUES (1.5, 1e308, 1000000000000000.0, 'a');
```

## Queries

```sql
SELECT (NULL NOT LIKE 'a*'), (NULL << a2.c2) FROM t0 AS a0, t0 AS a1 LEFT JOIN t0 AS a2 ON (-2.5 BETWEEN 0.1 AND ('_' COLLATE BINARY)) GROUP BY (NULL NOT LIKE 'a*'), (NULL << a2.c2);
SELECT (NULL NOT LIKE 'a*'), (NULL << a2.c2) FROM t0 AS a0, t0 AS a1 LEFT JOIN t0 AS a2 ON (-2.5 BETWEEN 0.1 AND ('_' COLLATE BINARY)) WHERE (((100 NOTNULL) >> (CASE WHEN (NOT (-2.5 IN (a1.c2))) THEN x'0102' END)) >= a2.c0) GROUP BY (NULL NOT LIKE 'a*'), (NULL << a2.c2) UNION SELECT (NULL NOT LIKE 'a*'), (NULL << a2.c2) FROM t0 AS a0, t0 AS a1 LEFT JOIN t0 AS a2 ON (-2.5 BETWEEN 0.1 AND ('_' COLLATE BINARY)) WHERE (NOT (((100 NOTNULL) >> (CASE WHEN (NOT (-2.5 IN (a1.c2))) THEN x'0102' END)) >= a2.c0)) GROUP BY (NULL NOT LIKE 'a*'), (NULL << a2.c2) UNION SELECT (NULL NOT LIKE 'a*'), (NULL << a2.c2) FROM t0 AS a0, t0 AS a1 LEFT JOIN t0 AS a2 ON (-2.5 BETWEEN 0.1 AND ('_' COLLATE BINARY)) WHERE ((((100 NOTNULL) >> (CASE WHEN (NOT (-2.5 IN (a1.c2))) THEN x'0102' END)) >= a2.c0) ISNULL) GROUP BY (NULL NOT LIKE 'a*'), (NULL << a2.c2);
```

## What differed

```
the query's rows aren't the UNION of its partitions'
  expected: 1 rows
    NULL, NULL
  Amphora:  0 rows
```
