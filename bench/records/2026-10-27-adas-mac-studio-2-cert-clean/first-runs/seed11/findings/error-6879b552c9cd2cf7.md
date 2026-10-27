# pqs Error: Amphora errors where SQLite finds the pivot: Parse error: bad JSON path: ''

seed 11, state 22888

## Statements

```sql
CREATE TABLE t0(c0 REAL COLLATE RTRIM NOT NULL, c1 BOOLEAN PRIMARY KEY);
INSERT OR IGNORE INTO t0 VALUES (0.0, ''), ('a ', '-1'), ('-1', 255);
INSERT OR IGNORE INTO t0 VALUES (' ', '1e3'), (-9223372036854775808, x'41');
INSERT OR IGNORE INTO t0 VALUES (' ', 'true'), (0.1, '1');
INSERT INTO t0 VALUES (' a', 'x');
INSERT OR IGNORE INTO t0 VALUES (x'', '10');
INSERT OR IGNORE INTO t0 VALUES (1, 1.5), (9007199254740993, -9223372036854775808);
INSERT INTO t0 VALUES (1.5, 1000000000000000.0);
ANALYZE;
```

## Queries

```sql
SELECT ((SELECT min(s257.c1) FROM t0 AS s257 WHERE (s257.c1 IN ((- 1000000000000000.0), quote(s257.c1), jsonb_extract(x'00', ''))))) IS TRUE, ((SELECT min(s257.c1) FROM t0 AS s257 WHERE (s257.c1 IN ((- 1000000000000000.0), quote(s257.c1), jsonb_extract(x'00', ''))))) IS FALSE FROM t0 AS a0 WHERE a0.rowid = 12;
SELECT a0.rowid FROM t0 AS a0 WHERE (SELECT min(s257.c1) FROM t0 AS s257 WHERE (s257.c1 IN ((- 1000000000000000.0), quote(s257.c1), jsonb_extract(x'00', ''))));
```

## What differed

```
Amphora errors where SQLite finds the pivot: Parse error: bad JSON path: ''
```
