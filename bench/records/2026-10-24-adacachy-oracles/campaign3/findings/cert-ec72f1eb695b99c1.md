# cert Estimate: the more restrictive query is estimated at 1000000 rows, the query at 547151.776468577

seed 13, state 20272

## Statements

```sql
CREATE TABLE t0(c0 INT, c1 INT COLLATE RTRIM, c2 BOOLEAN, c3 BLOB COLLATE NOCASE);
INSERT OR IGNORE INTO t0 VALUES ('1', '%', -9223372036854775808, 0);
CREATE TABLE t1(c0 VARCHAR(10) PRIMARY KEY, c1 INTEGER);
ANALYZE;
```

## Queries

```sql
SELECT x'c3bf' FROM t1 AS a0 FULL JOIN t0 AS a1 ON (x'c3bf' IN (SELECT s849.c0 FROM t1 AS s849)) WHERE (NULL BETWEEN x'41' AND a1.c2);
SELECT x'c3bf' FROM t1 AS a0 FULL JOIN t0 AS a1 ON (x'c3bf' IN (SELECT s849.c0 FROM t1 AS s849)) WHERE ((NULL BETWEEN x'41' AND a1.c2) AND 0);
```

## What differed

```
the more restrictive query is estimated at 1000000 rows, the query at 547151.776468577
```
