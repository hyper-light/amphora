# tlp Wrong: the query's rows aren't its three partitions' (p, NOT p, p IS NULL)

seed 20261024, state 953

## Statements

```sql
CREATE TABLE t0(c0 VARCHAR(10) PRIMARY KEY, c1 INT COLLATE RTRIM, c2 DOUBLE COLLATE NOCASE);
INSERT OR IGNORE INTO t0 VALUES (x'41', 3, ' ');
CREATE TABLE t1(c0 INTEGER NOT NULL, c1 BOOLEAN, c2 INT);
```

## Queries

```sql
SELECT json_extract('{"a":null}', '$.b[1]') FROM t0 AS a0, t1 AS a1 FULL JOIN t0 AS a2 ON (a0.c0 < (a0.c1 = 0.5));
SELECT json_extract('{"a":null}', '$.b[1]') FROM t0 AS a0, t1 AS a1 FULL JOIN t0 AS a2 ON (a0.c0 < (a0.c1 = 0.5)) WHERE (((CASE WHEN (2.0 NOT IN (9007199254740993, a2.c2, '1e3')) THEN a1.c2 ELSE -2.5 END) NOT GLOB 'abc') AND ((100 <= a0.c1) IN ((a0.c2 IS NOT DISTINCT FROM a2.c1))));
SELECT json_extract('{"a":null}', '$.b[1]') FROM t0 AS a0, t1 AS a1 FULL JOIN t0 AS a2 ON (a0.c0 < (a0.c1 = 0.5)) WHERE (NOT (((CASE WHEN (2.0 NOT IN (9007199254740993, a2.c2, '1e3')) THEN a1.c2 ELSE -2.5 END) NOT GLOB 'abc') AND ((100 <= a0.c1) IN ((a0.c2 IS NOT DISTINCT FROM a2.c1)))));
SELECT json_extract('{"a":null}', '$.b[1]') FROM t0 AS a0, t1 AS a1 FULL JOIN t0 AS a2 ON (a0.c0 < (a0.c1 = 0.5)) WHERE ((((CASE WHEN (2.0 NOT IN (9007199254740993, a2.c2, '1e3')) THEN a1.c2 ELSE -2.5 END) NOT GLOB 'abc') AND ((100 <= a0.c1) IN ((a0.c2 IS NOT DISTINCT FROM a2.c1)))) ISNULL);
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
