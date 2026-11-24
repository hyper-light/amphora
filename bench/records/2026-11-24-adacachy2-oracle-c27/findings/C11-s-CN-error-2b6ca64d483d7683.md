# tlp Error: a partition errors on Amphora, not on SQLite: Parse error: row value misused

seed 27004, state 1579

## Statements

```sql
CREATE TABLE t2(c0  NOT NULL, c1 NUMERIC COLLATE BINARY, c2 BOOLEAN, c3 VARCHAR(10) PRIMARY KEY);
```

## Queries

```sql
SELECT ' a', NULL FROM t2 AS a0 WHERE ((NULL, NULL, '1') NOT IN (SELECT s496.c0, s496.c2, s496.c0 FROM t2 AS s496 WHERE (s496.c2 < (CASE WHEN (s496.c2 ISNULL) THEN 0 WHEN (s496.c0 NOTNULL) THEN '0' ELSE 'a' END)))) GROUP BY ' a';
SELECT ' a', NULL FROM t2 AS a0 WHERE ((NULL, NULL, '1') NOT IN (SELECT s496.c0, s496.c2, s496.c0 FROM t2 AS s496 WHERE (s496.c2 < (CASE WHEN (s496.c2 ISNULL) THEN 0 WHEN (s496.c0 NOTNULL) THEN '0' ELSE 'a' END)))) GROUP BY ' a' HAVING x'41';
SELECT ' a', NULL FROM t2 AS a0 WHERE ((NULL, NULL, '1') NOT IN (SELECT s496.c0, s496.c2, s496.c0 FROM t2 AS s496 WHERE (s496.c2 < (CASE WHEN (s496.c2 ISNULL) THEN 0 WHEN (s496.c0 NOTNULL) THEN '0' ELSE 'a' END)))) GROUP BY ' a' HAVING (NOT x'41');
SELECT ' a', NULL FROM t2 AS a0 WHERE ((NULL, NULL, '1') NOT IN (SELECT s496.c0, s496.c2, s496.c0 FROM t2 AS s496 WHERE (s496.c2 < (CASE WHEN (s496.c2 ISNULL) THEN 0 WHEN (s496.c0 NOTNULL) THEN '0' ELSE 'a' END)))) GROUP BY ' a' HAVING (x'41' ISNULL);
```

## What differed

```
a partition errors on Amphora, not on SQLite: Parse error: row value misused
```
