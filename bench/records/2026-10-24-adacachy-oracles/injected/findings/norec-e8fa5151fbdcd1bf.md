# norec Wrong: WHERE p keeps 0 rows; p is TRUE for 1 of them

seed 20261024, state 1557

## Statements

```sql
CREATE TABLE t0(c0 TEXT, c1 VARCHAR(10), c2 BLOB COLLATE NOCASE, c3 INT NOT NULL);
INSERT OR IGNORE INTO t0 VALUES (100, 2.0, NULL, -100);
CREATE TABLE t2(c0 INTEGER COLLATE NOCASE);
```

## Queries

```sql
SELECT 1 FROM t2 AS a0 RIGHT JOIN t0 AS a1 ON NULL WHERE (a0.c0 ISNULL);
SELECT (CASE WHEN (a0.c0 ISNULL) THEN 1 ELSE 0 END) FROM t2 AS a0 RIGHT JOIN t0 AS a1 ON NULL;
```

## What differed

```
WHERE p keeps 0 rows; p is TRUE for 1 of them
```
