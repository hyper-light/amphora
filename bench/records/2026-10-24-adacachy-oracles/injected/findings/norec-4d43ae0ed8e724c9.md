# norec Wrong: WHERE p keeps 1 rows; p is TRUE for 2 of them

seed 20261024, state 102

## Statements

```sql
CREATE TABLE t0(c0 DOUBLE);
INSERT OR IGNORE INTO t0 VALUES (NULL);
```

## Queries

```sql
SELECT 1 FROM t0 AS a0 FULL JOIN t0 AS a1 ON NULL WHERE (a0.c0 ISNULL);
SELECT (CASE WHEN (a0.c0 ISNULL) THEN 1 ELSE 0 END) FROM t0 AS a0 FULL JOIN t0 AS a1 ON NULL;
```

## What differed

```
WHERE p keeps 1 rows; p is TRUE for 2 of them
```
