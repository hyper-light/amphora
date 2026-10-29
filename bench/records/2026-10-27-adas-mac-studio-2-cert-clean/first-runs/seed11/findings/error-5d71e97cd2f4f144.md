# pqs Error: Amphora errors where SQLite finds the pivot: JSON path error near: "1000000000000000"

seed 11, state 60510

## Statements

```sql
CREATE TABLE t1(c0 BOOLEAN PRIMARY KEY, c1 , c2 REAL NOT NULL, c3 INTEGER);
INSERT INTO t1 VALUES (1000000000000000.0, 0.0, 0.0, 0.5), (255, x'0102', 'x', NULL);
```

## Queries

```sql
SELECT ((SELECT min(s223.c0) FROM t1 AS s223 WHERE (json_set(s223.c3, s223.c0, s223.c1) ISNULL))) IS TRUE, ((SELECT min(s223.c0) FROM t1 AS s223 WHERE (json_set(s223.c3, s223.c0, s223.c1) ISNULL))) IS FALSE FROM t1 AS a0 WHERE a0.rowid = 1;
SELECT a0.rowid FROM t1 AS a0 WHERE (SELECT min(s223.c0) FROM t1 AS s223 WHERE (json_set(s223.c3, s223.c0, s223.c1) ISNULL));
```

## What differed

```
Amphora errors where SQLite finds the pivot: JSON path error near: "1000000000000000"
```
