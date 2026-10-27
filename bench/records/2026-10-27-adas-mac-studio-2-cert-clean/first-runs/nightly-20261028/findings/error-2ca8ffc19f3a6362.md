# pqs Error: Amphora errors where SQLite finds the pivot: Parse error: Unexpected character

seed 20261028, state 77658

## Statements

```sql
CREATE TABLE t1(c0 DECIMAL(10,2) PRIMARY KEY, c1 );
INSERT INTO t1 VALUES ('1e3', 1e308);
INSERT OR IGNORE INTO t1 VALUES (x'0102', 10);
INSERT INTO t1 VALUES (3, -100), (1.5, '_');
ANALYZE;
```

## Queries

```sql
SELECT ((a0.c0 BETWEEN json(a0.c0) AND '{"a":1,"b":[2,3]}')) IS TRUE, ((a0.c0 BETWEEN json(a0.c0) AND '{"a":1,"b":[2,3]}')) IS FALSE FROM t1 AS a0 WHERE a0.rowid = 4;
SELECT a0.rowid FROM t1 AS a0 WHERE (a0.c0 BETWEEN json(a0.c0) AND '{"a":1,"b":[2,3]}');
```

## What differed

```
Amphora errors where SQLite finds the pivot: Parse error: Unexpected character
```
