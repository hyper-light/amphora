# tlp Wrong: the query's aggregate isn't its partitions' combined

seed 20261024, state 553

## Statements

```sql
CREATE TABLE t0(c0 VARCHAR(10) NOT NULL, c1 DOUBLE PRIMARY KEY);
CREATE TABLE t2(c0 VARCHAR(10) COLLATE RTRIM, c1 INT COLLATE RTRIM UNIQUE, c2  UNIQUE, c3 NUMERIC);
```

## Queries

```sql
SELECT (255 COLLATE BINARY) FROM t2 AS a0 LEFT JOIN t0 AS a1 ON (CAST(a1.c0 AS REAL) BETWEEN (- a1.c1) AND (a0.c2 IS NOT DISTINCT FROM a0.c0)), t2 AS a2;
SELECT max(m COLLATE BINARY) FROM (SELECT NULL AS m WHERE 0 UNION ALL SELECT (255 COLLATE BINARY) FROM t2 AS a0 LEFT JOIN t0 AS a1 ON (CAST(a1.c0 AS REAL) BETWEEN (- a1.c1) AND (a0.c2 IS NOT DISTINCT FROM a0.c0)), t2 AS a2 WHERE ((- (a0.c2 < a2.c2)) <> a2.c2) UNION ALL SELECT (255 COLLATE BINARY) FROM t2 AS a0 LEFT JOIN t0 AS a1 ON (CAST(a1.c0 AS REAL) BETWEEN (- a1.c1) AND (a0.c2 IS NOT DISTINCT FROM a0.c0)), t2 AS a2 WHERE (NOT ((- (a0.c2 < a2.c2)) <> a2.c2)) UNION ALL SELECT (255 COLLATE BINARY) FROM t2 AS a0 LEFT JOIN t0 AS a1 ON (CAST(a1.c0 AS REAL) BETWEEN (- a1.c1) AND (a0.c2 IS NOT DISTINCT FROM a0.c0)), t2 AS a2 WHERE (((- (a0.c2 < a2.c2)) <> a2.c2) ISNULL));
```

## What differed

```
the query's aggregate isn't its partitions' combined
  expected: 0 rows
  Amphora:  1 rows
    NULL
```
