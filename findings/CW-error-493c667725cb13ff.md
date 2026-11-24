# pqs Error: Amphora errors where SQLite finds the pivot: Parse error: malformed JSON

seed 8003, state 7788

## Statements

```sql
CREATE TABLE t0(c0 INTEGER COLLATE NOCASE, c1 BLOB COLLATE BINARY, c2 REAL UNIQUE NOT NULL, c3 NUMERIC COLLATE BINARY UNIQUE);
INSERT OR IGNORE INTO t0 VALUES (x'c3bf', 'A', '', 1e308), ('-1', -1, 100, 'ABC'), (x'00', NULL, 10, -1e-308);
INSERT INTO t0 VALUES (0.1, 'ABC', x'41', '0');
CREATE VIEW v1 AS SELECT jsonb(x'c3bf') AS c0, (- -1e-308) AS c1, (+ cos(b66x0.c2)) AS c2 FROM t0 AS b66x0;
```

## Queries

```sql
SELECT ((EXISTS (SELECT 1 FROM v1 AS s712 WHERE (a0.c2 != '_')))) IS TRUE, ((EXISTS (SELECT 1 FROM v1 AS s712 WHERE (a0.c2 != '_')))) IS FALSE FROM t0 AS a0 WHERE a0.rowid = 4;
SELECT a0.rowid FROM t0 AS a0 WHERE (EXISTS (SELECT 1 FROM v1 AS s712 WHERE (a0.c2 != '_')));
```

## What differed

```
Amphora errors where SQLite finds the pivot: Parse error: malformed JSON
```
