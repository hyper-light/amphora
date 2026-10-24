# norec Wrong: WHERE p keeps 0 rows; p is TRUE for 1 of them

seed 20261024, state 913

## Statements

```sql
CREATE TABLE t0(c0 DECIMAL(10,2) PRIMARY KEY, c1 DOUBLE UNIQUE, c2  COLLATE NOCASE, c3 INT);
INSERT OR IGNORE INTO t0 VALUES ('ABC', x'41', x'00', 10);
```

## Queries

```sql
SELECT 1 FROM t0 AS a0 FULL JOIN t0 AS a1 ON NULL WHERE (a1.c0 ISNULL);
SELECT (CASE WHEN (a1.c0 ISNULL) THEN 1 ELSE 0 END) FROM t0 AS a0 FULL JOIN t0 AS a1 ON NULL;
```

## What differed

```
WHERE p keeps 0 rows; p is TRUE for 1 of them
```
