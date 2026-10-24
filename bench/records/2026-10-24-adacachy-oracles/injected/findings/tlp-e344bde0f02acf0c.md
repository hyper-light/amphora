# tlp Wrong: the query's rows aren't the UNION of its partitions'

seed 20261024, state 1135

## Statements

```sql
CREATE TABLE t0(c0 TEXT, c1  PRIMARY KEY);
INSERT OR IGNORE INTO t0 VALUES (7, NULL);
CREATE TABLE t1(c0 VARCHAR(10) UNIQUE, c1 DOUBLE, c2 DECIMAL(10,2), c3 DOUBLE COLLATE BINARY);
```

## Queries

```sql
SELECT (NULL NOTNULL), json_extract('[1,2,3]', '$.a') FROM t1 AS a0, t0 AS a1 FULL JOIN t0 AS a2 ON (a1.c1 NOTNULL) GROUP BY (NULL NOTNULL), json_extract('[1,2,3]', '$.a');
SELECT (NULL NOTNULL), json_extract('[1,2,3]', '$.a') FROM t1 AS a0, t0 AS a1 FULL JOIN t0 AS a2 ON (a1.c1 NOTNULL) WHERE ((NOT (NOT (a1.c1 == a0.c0))) AND ((a0.c3 ISNULL) AND (NOT (a0.c2 BETWEEN 'A' AND a1.c1)))) GROUP BY (NULL NOTNULL), json_extract('[1,2,3]', '$.a') UNION SELECT (NULL NOTNULL), json_extract('[1,2,3]', '$.a') FROM t1 AS a0, t0 AS a1 FULL JOIN t0 AS a2 ON (a1.c1 NOTNULL) WHERE (NOT ((NOT (NOT (a1.c1 == a0.c0))) AND ((a0.c3 ISNULL) AND (NOT (a0.c2 BETWEEN 'A' AND a1.c1))))) GROUP BY (NULL NOTNULL), json_extract('[1,2,3]', '$.a') UNION SELECT (NULL NOTNULL), json_extract('[1,2,3]', '$.a') FROM t1 AS a0, t0 AS a1 FULL JOIN t0 AS a2 ON (a1.c1 NOTNULL) WHERE (((NOT (NOT (a1.c1 == a0.c0))) AND ((a0.c3 ISNULL) AND (NOT (a0.c2 BETWEEN 'A' AND a1.c1)))) ISNULL) GROUP BY (NULL NOTNULL), json_extract('[1,2,3]', '$.a');
```

## What differed

```
the query's rows aren't the UNION of its partitions'
  expected: 1 rows
    0, NULL
  Amphora:  0 rows
```
