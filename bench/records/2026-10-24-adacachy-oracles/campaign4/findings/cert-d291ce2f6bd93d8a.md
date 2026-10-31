# cert Estimate: the more restrictive query is estimated at 10 rows, the query at 4

seed 14, state 687

## Statements

```sql
CREATE TABLE t0(c0 REAL COLLATE RTRIM, c1 INTEGER PRIMARY KEY);
INSERT INTO t0 VALUES (' a', '10'), (0.0, 2);
INSERT INTO t0 VALUES (1000000000000000.0, 1);
INSERT OR IGNORE INTO t0 VALUES (NULL, NULL);
CREATE TABLE t1(c0 INT COLLATE BINARY);
INSERT OR IGNORE INTO t1 VALUES ('A');
INSERT INTO t1 VALUES (x'00'), (3);
INSERT INTO t1 VALUES (1.5), (NULL), (-1e-308);
INSERT INTO t1 VALUES (' '), (100);
INSERT INTO t1 VALUES (-9223372036854775808), (x'41');
ANALYZE;
```

## Queries

```sql
SELECT NULL FROM t1 AS a0 CROSS JOIN t0 AS a1 WHERE (a1.c0 IS 100);
SELECT NULL FROM t1 AS a0 CROSS JOIN t0 AS a1 WHERE ((a1.c0 IS 100) AND (likely(NULL) NOT BETWEEN json_extract('"x"', '$[0]') AND CAST(a1.c0 AS NUMERIC)));
```

## What differed

```
the more restrictive query is estimated at 10 rows, the query at 4
```
