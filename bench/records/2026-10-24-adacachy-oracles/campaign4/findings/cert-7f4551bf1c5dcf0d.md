# cert Estimate: the more restrictive query is estimated at 2 rows, the query at 1.2493289641172216

seed 14, state 10068

## Statements

```sql
CREATE TABLE t0(c0 DECIMAL(10,2), c1 DOUBLE COLLATE BINARY PRIMARY KEY);
INSERT INTO t0 VALUES (1, NULL), (10, 2);
CREATE TABLE t2(c0 REAL COLLATE RTRIM, c1 , c2 DECIMAL(10,2) UNIQUE, c3 NUMERIC PRIMARY KEY);
INSERT OR IGNORE INTO t2 VALUES ('ABC', ' a', '%', 3);
CREATE INDEX i2 ON t2(c0);
ANALYZE;
```

## Queries

```sql
SELECT NULL FROM t2 AS a0 LEFT JOIN t0 AS a1 ON (a0.c0 <= a1.c0);
SELECT NULL FROM t2 AS a0 JOIN t0 AS a1 ON (a0.c0 <= a1.c0);
```

## What differed

```
the more restrictive query is estimated at 2 rows, the query at 1.2493289641172216
```
