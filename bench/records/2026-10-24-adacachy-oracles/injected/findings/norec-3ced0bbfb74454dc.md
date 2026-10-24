# norec Wrong: WHERE p keeps 2 rows; p is TRUE for 0 of them

seed 20261024, state 831

## Statements

```sql
CREATE TABLE t0(c0 NUMERIC COLLATE BINARY UNIQUE);
INSERT INTO t0 VALUES ('1e3'), (-9223372036854775808);
```

## Queries

```sql
SELECT 1 FROM t0 AS a0, t0 AS a1 WHERE (NOT ((('a' BETWEEN 2.0 AND NULL) AND (a1.c0 IS 2)) OR (a1.c0 == a0.c0)));
SELECT (CASE WHEN (NOT ((('a' BETWEEN 2.0 AND NULL) AND (a1.c0 IS 2)) OR (a1.c0 == a0.c0))) THEN 1 ELSE 0 END) FROM t0 AS a0, t0 AS a1;
```

## What differed

```
WHERE p keeps 2 rows; p is TRUE for 0 of them
```
