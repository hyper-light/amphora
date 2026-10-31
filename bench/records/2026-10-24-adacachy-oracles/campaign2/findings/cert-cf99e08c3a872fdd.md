# cert Estimate: the more restrictive query is estimated at 0.5 rows, the query at 0.05

seed 12, state 1580

## Statements

```sql
CREATE TABLE t0(c0 BLOB, c1 INTEGER, c2 INT);
INSERT OR IGNORE INTO t0 VALUES (NULL, x'00', 'A'), (x'41', 0, 2.0);
INSERT OR IGNORE INTO t0 VALUES (-1e-308, 'true', '10'), (-9223372036854775808, 100, -1e-308), ('a', 2.0, '1e3');
CREATE TABLE t1(c0 VARCHAR(10) UNIQUE, c1 TEXT, c2 TEXT PRIMARY KEY);
INSERT INTO t1 VALUES ('', '1.5', ' ');
ANALYZE;
```

## Queries

```sql
SELECT ((a0.c1 <= -9223372036854775808) OR (NULL > 9223372036854775807)) FROM t0 AS a0 JOIN t1 AS a1 ON ((a1.c1 IS NOT DISTINCT FROM a0.c2) AND (SELECT min(s339.c1) FROM t0 AS s339 WHERE '0')), t1 AS a2 WHERE ((NULL BETWEEN a1.c1 AND a1.c2) IS (CASE CAST(a0.c1 AS REAL) WHEN (NULL >= '') THEN a1.c1 END));
SELECT ((a0.c1 <= -9223372036854775808) OR (NULL > 9223372036854775807)) FROM t0 AS a0 JOIN t1 AS a1 ON ((a1.c1 IS NOT DISTINCT FROM a0.c2) AND (SELECT min(s339.c1) FROM t0 AS s339 WHERE '0')), t1 AS a2 WHERE (((NULL BETWEEN a1.c1 AND a1.c2) IS (CASE CAST(a0.c1 AS REAL) WHEN (NULL >= '') THEN a1.c1 END)) AND (NOT (a1.c0 OR (a2.c1 <= a0.c2))));
```

## What differed

```
the more restrictive query is estimated at 0.5 rows, the query at 0.05
```
