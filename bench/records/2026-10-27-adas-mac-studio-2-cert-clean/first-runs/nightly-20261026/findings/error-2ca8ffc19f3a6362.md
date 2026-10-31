# pqs Error: Amphora errors where SQLite finds the pivot: Parse error: Unexpected character

seed 20261026, state 31590

## Statements

```sql
CREATE TABLE t0(c0 REAL, c1 BLOB, c2 REAL PRIMARY KEY);
INSERT INTO t0 VALUES ('1', -1e-308, '-1'), (100, '0x10', ' '), (9007199254740993, x'c3bf', 'x');
INSERT OR IGNORE INTO t0 VALUES (x'', 2, ''), ('', 'a ', NULL), ('%', '1.5', 100);
INSERT INTO t0 VALUES (100, '-1', 'ABC'), (NULL, 10, 7);
CREATE INDEX i0 ON t0(c1 COLLATE RTRIM DESC, c1 DESC);
```

## Queries

```sql
SELECT ((EXISTS (SELECT 1 FROM t0 AS s197 WHERE (jsonb_insert(s197.c1) IS NOT (s197.c1 << -100))))) IS TRUE, ((EXISTS (SELECT 1 FROM t0 AS s197 WHERE (jsonb_insert(s197.c1) IS NOT (s197.c1 << -100))))) IS FALSE FROM t0 AS a0 WHERE a0.rowid = 8;
SELECT a0.rowid FROM t0 AS a0 WHERE (EXISTS (SELECT 1 FROM t0 AS s197 WHERE (jsonb_insert(s197.c1) IS NOT (s197.c1 << -100))));
```

## What differed

```
Amphora errors where SQLite finds the pivot: Parse error: Unexpected character
```
