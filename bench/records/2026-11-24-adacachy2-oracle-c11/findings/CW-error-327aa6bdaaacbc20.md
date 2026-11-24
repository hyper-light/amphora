# pqs Error: Amphora errors where SQLite finds the pivot: JSON path error near: "3"

seed 8003, state 8957

## Statements

```sql
CREATE TABLE t1(c0 NUMERIC PRIMARY KEY);
INSERT OR IGNORE INTO t1 VALUES (3.0);
CREATE VIEW v1 AS SELECT DISTINCT CAST(b17x0.c0 AS INTEGER) AS c0 FROM t1 AS b17x0 LEFT JOIN t1 AS b17x1 ON (json_remove(1e308, b17x0.c0) NOTNULL) WHERE (NOT (b17x0.c0 GLOB '[a-z]*'));
```

## Queries

```sql
SELECT ((EXISTS (SELECT 1 FROM v1 AS s609))) IS TRUE, ((EXISTS (SELECT 1 FROM v1 AS s609))) IS FALSE FROM t1 AS a0 WHERE a0.rowid = 1;
SELECT a0.rowid FROM t1 AS a0 WHERE (EXISTS (SELECT 1 FROM v1 AS s609));
```

## What differed

```
Amphora errors where SQLite finds the pivot: JSON path error near: "3"
```
