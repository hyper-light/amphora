# norec Wrong: WHERE p keeps 0 rows; p is TRUE for 1 of them

seed 20261024, state 922

## Statements

```sql
CREATE TABLE t0(c0  COLLATE BINARY NOT NULL);
INSERT OR IGNORE INTO t0 VALUES (0.5);
CREATE TABLE t1(c0 NUMERIC, c1  PRIMARY KEY NOT NULL, c2 VARCHAR(10));
```

## Queries

```sql
SELECT 1 FROM t1 AS a0, t1 AS a1 RIGHT JOIN t0 AS a2 ON ((a0.c0 COLLATE BINARY) NOT BETWEEN (a2.c0 COLLATE NOCASE) AND (- a2.c0)) WHERE (((a2.c0 || a0.c2) | (CASE WHEN (a2.c0 IS NOT a2.c0) THEN a2.c0 WHEN a2.c0 THEN a1.c1 END)) ISNULL);
SELECT (CASE WHEN (((a2.c0 || a0.c2) | (CASE WHEN (a2.c0 IS NOT a2.c0) THEN a2.c0 WHEN a2.c0 THEN a1.c1 END)) ISNULL) THEN 1 ELSE 0 END) FROM t1 AS a0, t1 AS a1 RIGHT JOIN t0 AS a2 ON ((a0.c0 COLLATE BINARY) NOT BETWEEN (a2.c0 COLLATE NOCASE) AND (- a2.c0));
```

## What differed

```
WHERE p keeps 0 rows; p is TRUE for 1 of them
```
