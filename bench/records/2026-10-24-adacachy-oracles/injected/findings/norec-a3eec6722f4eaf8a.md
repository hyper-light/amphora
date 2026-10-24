# norec Wrong: WHERE p keeps 0 rows; p is TRUE for 1 of them

seed 20261024, state 581

## Statements

```sql
CREATE TABLE t0(c0 NUMERIC UNIQUE, c1 TEXT, c2 BLOB UNIQUE, c3 BLOB);
CREATE TABLE t1(c0 REAL, c1 BOOLEAN);
INSERT OR IGNORE INTO t1 VALUES (NULL, 9007199254740993);
```

## Queries

```sql
SELECT 1 FROM t0 AS a0 RIGHT JOIN t1 AS a1 ON 10 WHERE (a0.c0 ISNULL);
SELECT (CASE WHEN (a0.c0 ISNULL) THEN 1 ELSE 0 END) FROM t0 AS a0 RIGHT JOIN t1 AS a1 ON 10;
```

## What differed

```
WHERE p keeps 0 rows; p is TRUE for 1 of them
```
