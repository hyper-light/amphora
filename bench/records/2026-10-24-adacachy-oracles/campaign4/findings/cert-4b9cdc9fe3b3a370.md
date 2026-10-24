# cert Estimate: the more restrictive query is estimated at 2 rows, the query at 1.018730753077982

seed 14, state 20790

## Statements

```sql
CREATE TABLE t0(c0 INTEGER, c1 VARCHAR(10) COLLATE BINARY);
INSERT INTO t0 VALUES (9223372036854775807, 'a '), (' ', 10);
CREATE TABLE t1(c0 REAL, c1 BLOB, c2 INTEGER, c3 DOUBLE PRIMARY KEY);
INSERT OR IGNORE INTO t1 VALUES (NULL, '0', -2.5, x'00');
ANALYZE;
```

## Queries

```sql
SELECT '_' FROM t1 AS a0 FULL JOIN t0 AS a1 ON (a1.c0 = a0.c3);
SELECT '_' FROM t1 AS a0 JOIN t0 AS a1 ON (a1.c0 = a0.c3);
```

## What differed

```
the more restrictive query is estimated at 2 rows, the query at 1.018730753077982
```
