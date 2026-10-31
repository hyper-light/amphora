# pqs Error: Amphora errors where SQLite finds the pivot: json_object() labels must be TEXT

seed 11, state 65372

## Statements

```sql
CREATE TABLE t1(c0 VARCHAR(10) COLLATE BINARY, c1 BOOLEAN, c2 DOUBLE, c3 REAL);
INSERT OR IGNORE INTO t1 VALUES ('x', NULL, NULL, -100), ('_', -1, -1e-308, '');
INSERT INTO t1 VALUES (-100, -9223372036854775808, 0, '1.5'), ('1', '_', 2.0, x'00');
INSERT OR IGNORE INTO t1 VALUES (NULL, 9223372036854775807, 1e308, ' '), ('a ', 3, '1', 9007199254740993), ('x', -1, 1e308, 1000000000000000.0);
CREATE TABLE t2(c0 BOOLEAN PRIMARY KEY, c1  COLLATE BINARY, c2 VARCHAR(10) UNIQUE, c3 INTEGER);
INSERT INTO t2 VALUES (x'00', NULL, '0', NULL), (x'0102', 0.0, 'x', -9223372036854775808);
```

## Queries

```sql
SELECT ((SELECT max(s456.c0) FROM t2 AS s456 WHERE (s456.c1 IN (s456.c0, s456.c1, json_object(1e308, s456.c3))))) IS TRUE, ((SELECT max(s456.c0) FROM t2 AS s456 WHERE (s456.c1 IN (s456.c0, s456.c1, json_object(1e308, s456.c3))))) IS FALSE FROM t1 AS a0 WHERE a0.rowid = 7;
SELECT a0.rowid FROM t1 AS a0 WHERE (NOT (SELECT max(s456.c0) FROM t2 AS s456 WHERE (s456.c1 IN (s456.c0, s456.c1, json_object(1e308, s456.c3)))));
```

## What differed

```
Amphora errors where SQLite finds the pivot: json_object() labels must be TEXT
```
