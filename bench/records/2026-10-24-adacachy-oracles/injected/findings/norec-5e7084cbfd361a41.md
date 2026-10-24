# norec Wrong: WHERE p keeps 0 rows; p is TRUE for 1 of them

seed 20261024, state 372

## Statements

```sql
CREATE TABLE t0(c0 INTEGER PRIMARY KEY NOT NULL);
INSERT OR IGNORE INTO t0 VALUES (9007199254740993);
```

## Queries

```sql
SELECT 1 FROM t0 AS a0 LEFT JOIN t0 AS a1 ON NULL WHERE (a1.c0 ISNULL);
SELECT (CASE WHEN (a1.c0 ISNULL) THEN 1 ELSE 0 END) FROM t0 AS a0 LEFT JOIN t0 AS a1 ON NULL;
```

## What differed

```
WHERE p keeps 0 rows; p is TRUE for 1 of them
```
