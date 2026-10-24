# norec Wrong: WHERE p keeps 2 rows; p is TRUE for 0 of them

seed 20261024, state 615

## Statements

```sql
CREATE TABLE t0(c0 INT COLLATE NOCASE NOT NULL, c1 VARCHAR(10), c2 DECIMAL(10,2) COLLATE NOCASE, c3 TEXT COLLATE RTRIM);
INSERT OR IGNORE INTO t0 VALUES (x'41', x'ff', 3.0, '1.5');
CREATE TABLE t1(c0 INTEGER COLLATE NOCASE PRIMARY KEY);
INSERT INTO t1 VALUES (0);
```

## Queries

```sql
SELECT 1 FROM t1 AS a0 FULL JOIN t1 AS a1 ON ((3.0 IS a1.c0) OR (a0.c0 < a1.c0)), t0 AS a2 WHERE (NOT ((a0.c0 BETWEEN a1.c0 AND a0.c0) AND (a1.c0 IS NOT DISTINCT FROM a2.c1)));
SELECT (CASE WHEN (NOT ((a0.c0 BETWEEN a1.c0 AND a0.c0) AND (a1.c0 IS NOT DISTINCT FROM a2.c1))) THEN 1 ELSE 0 END) FROM t1 AS a0 FULL JOIN t1 AS a1 ON ((3.0 IS a1.c0) OR (a0.c0 < a1.c0)), t0 AS a2;
```

## What differed

```
WHERE p keeps 2 rows; p is TRUE for 0 of them
```
