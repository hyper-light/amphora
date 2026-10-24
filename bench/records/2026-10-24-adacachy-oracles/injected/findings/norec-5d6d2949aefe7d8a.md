# norec Wrong: WHERE p keeps 0 rows; p is TRUE for 1 of them

seed 20261024, state 829

## Statements

```sql
CREATE TABLE t0(c0 BLOB, c1 REAL, c2 REAL);
INSERT INTO t0 VALUES (x'41', 100, 9223372036854775807);
```

## Queries

```sql
SELECT 1 FROM t0 AS a0, t0 AS a1 FULL JOIN t0 AS a2 ON a0.c0 WHERE (CAST(a1.c1 AS NUMERIC) ISNULL);
SELECT (CASE WHEN (CAST(a1.c1 AS NUMERIC) ISNULL) THEN 1 ELSE 0 END) FROM t0 AS a0, t0 AS a1 FULL JOIN t0 AS a2 ON a0.c0;
```

## What differed

```
WHERE p keeps 0 rows; p is TRUE for 1 of them
```
