# tlp Wrong: the query's rows aren't its three partitions' (p, NOT p, p IS NULL)

seed 1011, state 38948

## Statements

```sql
CREATE TABLE t0(c0 REAL COLLATE RTRIM, c1 INTEGER PRIMARY KEY NOT NULL, c2 VARCHAR(10) UNIQUE);
INSERT OR IGNORE INTO t0 VALUES (255, NULL, 255);
CREATE INDEX i0 ON t0(c0 DESC);
```

## Queries

```sql
SELECT 0.0 FROM t0 AS a0 LEFT JOIN t0 AS a1 ON (a0.c2 <= 100) WHERE (100 < a0.c0);
SELECT 0.0 FROM t0 AS a0 LEFT JOIN t0 AS a1 ON (a0.c2 <= 100) WHERE ((100 < a0.c0) AND (NULL = a0.c1));
SELECT 0.0 FROM t0 AS a0 LEFT JOIN t0 AS a1 ON (a0.c2 <= 100) WHERE ((100 < a0.c0) AND (NOT (NULL = a0.c1)));
SELECT 0.0 FROM t0 AS a0 LEFT JOIN t0 AS a1 ON (a0.c2 <= 100) WHERE ((100 < a0.c0) AND ((NULL = a0.c1) ISNULL));
```

## What differed

```
the query's rows aren't its three partitions' (p, NOT p, p IS NULL)
  expected: 1 rows
    0.0
  Amphora:  0 rows
```
