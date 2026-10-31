# norec Wrong: WHERE p keeps 0 rows; p is TRUE for 1 of them

seed 20261024, state 1060

## Statements

```sql
CREATE TABLE t1(c0 INTEGER PRIMARY KEY, c1 DOUBLE COLLATE NOCASE, c2 TEXT, c3  COLLATE RTRIM);
INSERT INTO t1 VALUES (0.0, 'é', -1, 10);
CREATE TABLE t2(c0 INTEGER COLLATE RTRIM, c1 REAL);
```

## Queries

```sql
SELECT 1 FROM t1 AS a0 FULL JOIN t2 AS a2 ON NULL WHERE (a2.c0 ISNULL);
SELECT (CASE WHEN (a2.c0 ISNULL) THEN 1 ELSE 0 END) FROM t1 AS a0 FULL JOIN t2 AS a2 ON NULL;
```

## What differed

```
WHERE p keeps 0 rows; p is TRUE for 1 of them
```
