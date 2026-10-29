# pqs Error: Amphora errors where SQLite finds the pivot: JSON path error near: "3"

seed 13, state 68086

## Statements

```sql
CREATE TABLE t1(c0 INTEGER, c1 DOUBLE, c2 VARCHAR(10) COLLATE BINARY, c3 TEXT);
INSERT OR IGNORE INTO t1 VALUES (-1, 10, 'ABC', x'c3bf');
CREATE TABLE t2(c0 INTEGER COLLATE RTRIM PRIMARY KEY);
```

## Queries

```sql
SELECT ((EXISTS (SELECT 1 FROM t2 AS s84 WHERE (json_remove(-1e-308, 3) BETWEEN (+ s84.c0) AND 'é')))) IS TRUE, ((EXISTS (SELECT 1 FROM t2 AS s84 WHERE (json_remove(-1e-308, 3) BETWEEN (+ s84.c0) AND 'é')))) IS FALSE FROM t1 AS a0 WHERE a0.rowid = 1;
SELECT a0.rowid FROM t1 AS a0 WHERE (NOT (EXISTS (SELECT 1 FROM t2 AS s84 WHERE (json_remove(-1e-308, 3) BETWEEN (+ s84.c0) AND 'é'))));
```

## What differed

```
Amphora errors where SQLite finds the pivot: JSON path error near: "3"
```
