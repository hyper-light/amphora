# cert Estimate: the more restrictive query is estimated at 1000000 rows, the query at 10676.676416183063

seed 14, state 13808

## Statements

```sql
CREATE TABLE t0(c0 VARCHAR(10) PRIMARY KEY);
INSERT OR IGNORE INTO t0 VALUES (9223372036854775807);
INSERT INTO t0 VALUES (-1e-308);
CREATE TABLE t1(c0 VARCHAR(10) PRIMARY KEY);
ANALYZE;
```

## Queries

```sql
SELECT 1000000000000000.0 FROM t1 AS a0 RIGHT JOIN t0 AS a1 ON '-1' WHERE (a1.c0 BETWEEN 1.5 AND '[1,2,3]');
SELECT 1000000000000000.0 FROM t1 AS a0 JOIN t0 AS a1 ON '-1' WHERE (a1.c0 BETWEEN 1.5 AND '[1,2,3]');
```

## What differed

```
the more restrictive query is estimated at 1000000 rows, the query at 10676.676416183063
```
