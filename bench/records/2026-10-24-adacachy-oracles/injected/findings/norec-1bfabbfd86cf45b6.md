# norec Wrong: WHERE p keeps 0 rows; p is TRUE for 1 of them

seed 20261024, state 948

## Statements

```sql
CREATE TABLE t0(c0 DOUBLE COLLATE RTRIM NOT NULL, c1 INT, c2 VARCHAR(10), c3 TEXT NOT NULL);
INSERT OR IGNORE INTO t0 VALUES ('a ', 0.5, '0x10', x'');
```

## Queries

```sql
SELECT 1 FROM t0 AS a0 RIGHT JOIN t0 AS a1 ON NULL WHERE (a0.c2 ISNULL);
SELECT (CASE WHEN (a0.c2 ISNULL) THEN 1 ELSE 0 END) FROM t0 AS a0 RIGHT JOIN t0 AS a1 ON NULL;
```

## What differed

```
WHERE p keeps 0 rows; p is TRUE for 1 of them
```
