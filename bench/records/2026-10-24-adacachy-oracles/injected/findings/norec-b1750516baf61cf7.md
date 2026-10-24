# norec Wrong: WHERE p keeps 0 rows; p is TRUE for 1 of them

seed 20261024, state 426

## Statements

```sql
CREATE TABLE t0(c0 REAL, c1 REAL);
INSERT OR IGNORE INTO t0 VALUES (-2.5, 255);
CREATE TABLE t2(c0 REAL);
```

## Queries

```sql
SELECT 1 FROM t0 AS a0 LEFT JOIN t2 AS a1 ON NULL WHERE (a1.c0 ISNULL);
SELECT (CASE WHEN (a1.c0 ISNULL) THEN 1 ELSE 0 END) FROM t0 AS a0 LEFT JOIN t2 AS a1 ON NULL;
```

## What differed

```
WHERE p keeps 0 rows; p is TRUE for 1 of them
```
