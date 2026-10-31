# norec Wrong: WHERE p keeps 0 rows; p is TRUE for 1 of them

seed 20261024, state 634

## Statements

```sql
CREATE TABLE t1(c0 INT COLLATE RTRIM PRIMARY KEY, c1 BLOB UNIQUE);
INSERT INTO t1 VALUES (x'41', 'abc');
```

## Queries

```sql
SELECT 1 FROM t1 AS a0 FULL JOIN t1 AS a2 ON NULL WHERE (a2.c1 ISNULL);
SELECT (CASE WHEN (a2.c1 ISNULL) THEN 1 ELSE 0 END) FROM t1 AS a0 FULL JOIN t1 AS a2 ON NULL;
```

## What differed

```
WHERE p keeps 0 rows; p is TRUE for 1 of them
```
