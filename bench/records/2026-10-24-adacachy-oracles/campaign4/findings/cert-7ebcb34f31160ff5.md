# cert Estimate: the more restrictive query is estimated at 2 rows, the query at 1.3678794411714423

seed 14, state 15038

## Statements

```sql
CREATE TABLE t0(c0 INT COLLATE BINARY, c1 VARCHAR(10) COLLATE RTRIM, c2 NUMERIC UNIQUE, c3 NUMERIC UNIQUE);
INSERT INTO t0 VALUES ('abc', 'x', x'41', 'é');
CREATE TABLE t1(c0 INTEGER, c1 TEXT, c2 BOOLEAN COLLATE NOCASE PRIMARY KEY, c3 );
INSERT INTO t1 VALUES (NULL, '-1', 'abc', 9223372036854775807), (0.5, 'abc', 0.0, '1e3');
ANALYZE;
```

## Queries

```sql
SELECT NULL FROM t0 AS a0 RIGHT JOIN t1 AS a1 ON (a0.c3 = a1.c2);
SELECT NULL FROM t0 AS a0 JOIN t1 AS a1 ON (a0.c3 = a1.c2);
```

## What differed

```
the more restrictive query is estimated at 2 rows, the query at 1.3678794411714423
```
