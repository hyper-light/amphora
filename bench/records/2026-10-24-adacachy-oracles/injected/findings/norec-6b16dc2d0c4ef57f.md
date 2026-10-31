# norec Wrong: WHERE p keeps 0 rows; p is TRUE for 1 of them

seed 20261024, state 1280

## Statements

```sql
CREATE TABLE t0(c0 TEXT PRIMARY KEY);
CREATE TABLE t1(c0 INTEGER COLLATE NOCASE PRIMARY KEY, c1 INT, c2 DECIMAL(10,2));
INSERT OR IGNORE INTO t1 VALUES (2.0, 10, 255);
```

## Queries

```sql
SELECT 1 FROM t1 AS a0 LEFT JOIN t0 AS a1 ON ((a1.c0 | a1.c0) IN (x'41')), t1 AS a2 WHERE ((json_error_position(a2.c1) / a1.c0) ISNULL);
SELECT (CASE WHEN ((json_error_position(a2.c1) / a1.c0) ISNULL) THEN 1 ELSE 0 END) FROM t1 AS a0 LEFT JOIN t0 AS a1 ON ((a1.c0 | a1.c0) IN (x'41')), t1 AS a2;
```

## What differed

```
WHERE p keeps 0 rows; p is TRUE for 1 of them
```
