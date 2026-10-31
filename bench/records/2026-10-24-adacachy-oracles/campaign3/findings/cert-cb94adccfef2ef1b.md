# cert Estimate: the more restrictive query is estimated at 1000000 rows, the query at 0

seed 13, state 8176

## Statements

```sql
CREATE TABLE t0(c0 INT, c1 INTEGER, c2 TEXT UNIQUE);
CREATE TABLE t1(c0 BOOLEAN);
CREATE INDEX i0 ON t1(c0 DESC);
```

## Queries

```sql
SELECT '{"a":1,"b":[2,3]}' FROM t1 AS a0 RIGHT JOIN t0 AS a1 ON NULL WHERE (a0.c0 IN (SELECT s817.c0 FROM t1 AS s817 WHERE 'a '));
SELECT '{"a":1,"b":[2,3]}' FROM t1 AS a0 JOIN t0 AS a1 ON NULL WHERE (a0.c0 IN (SELECT s817.c0 FROM t1 AS s817 WHERE 'a '));
```

## What differed

```
the more restrictive query is estimated at 1000000 rows, the query at 0
```
