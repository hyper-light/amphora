# tlp Wrong: the query's aggregate isn't its partitions' combined

seed 20261024, state 1325

## Statements

```sql
CREATE TABLE t0(c0 BLOB COLLATE RTRIM, c1  COLLATE NOCASE, c2 BOOLEAN);
INSERT OR IGNORE INTO t0 VALUES ('-1', 'ABC', NULL);
CREATE TABLE t1(c0 DOUBLE UNIQUE, c1 NUMERIC UNIQUE, c2 DOUBLE, c3 TEXT COLLATE BINARY);
INSERT INTO t1 VALUES (0.5, 1.5, 'true', 'A');
```

## Queries

```sql
SELECT max((CAST((255 > 0.0) AS REAL) COLLATE BINARY)) FROM t0 AS a0 FULL JOIN t0 AS a1 ON (NOT (a0.c0 == a0.c1)), t1 AS a2;
SELECT max(m COLLATE BINARY) FROM (SELECT NULL AS m WHERE 0 UNION ALL SELECT max((CAST((255 > 0.0) AS REAL) COLLATE BINARY)) FROM t0 AS a0 FULL JOIN t0 AS a1 ON (NOT (a0.c0 == a0.c1)), t1 AS a2 WHERE ((NOT (NULL IN (a2.c3, NULL))) AND (SELECT max(s103.c3) FROM t1 AS s103 WHERE ((x'ff' IS NOT '1') OR 2.0))) UNION ALL SELECT max((CAST((255 > 0.0) AS REAL) COLLATE BINARY)) FROM t0 AS a0 FULL JOIN t0 AS a1 ON (NOT (a0.c0 == a0.c1)), t1 AS a2 WHERE (NOT ((NOT (NULL IN (a2.c3, NULL))) AND (SELECT max(s103.c3) FROM t1 AS s103 WHERE ((x'ff' IS NOT '1') OR 2.0)))) UNION ALL SELECT max((CAST((255 > 0.0) AS REAL) COLLATE BINARY)) FROM t0 AS a0 FULL JOIN t0 AS a1 ON (NOT (a0.c0 == a0.c1)), t1 AS a2 WHERE (((NOT (NULL IN (a2.c3, NULL))) AND (SELECT max(s103.c3) FROM t1 AS s103 WHERE ((x'ff' IS NOT '1') OR 2.0))) ISNULL));
```

## What differed

```
the query's aggregate isn't its partitions' combined
  expected: 1 rows
    1.0
  Amphora:  1 rows
    NULL
```
