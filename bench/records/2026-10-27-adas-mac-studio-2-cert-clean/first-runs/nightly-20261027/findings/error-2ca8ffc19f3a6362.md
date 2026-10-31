# pqs Error: Amphora errors where SQLite finds the pivot: Parse error: Unexpected character

seed 20261027, state 8820

## Statements

```sql
CREATE TABLE t0(c0 NUMERIC UNIQUE, c1 DOUBLE, c2 TEXT NOT NULL, c3 REAL);
INSERT INTO t0 VALUES (x'41', 3, '1.5', x'0102'), (9007199254740993, 2.0, x'c3bf', 0.5);
CREATE TABLE t2(c0 TEXT COLLATE BINARY, c1  UNIQUE, c2 REAL COLLATE RTRIM);
INSERT INTO t2 VALUES ('abc', NULL, x'00');
INSERT OR IGNORE INTO t2 VALUES ('1.5', 3, 'true'), (x'0102', -100, 1.5);
INSERT OR IGNORE INTO t2 VALUES (0.0, x'0102', 2);
INSERT OR IGNORE INTO t2 VALUES (-1e-308, NULL, x'0102'), (x'0102', -1e-308, 'a '), (3.0, '1e3', -2.5);
INSERT OR IGNORE INTO t2 VALUES ('0', ' a', x'0102'), ('-1', -1, 255), (x'00', 9007199254740993, '');
INSERT INTO t2 VALUES (' a', 'é', 10);
```

## Queries

```sql
SELECT ((SELECT max(s840.c0) FROM t0 AS s840 WHERE ((s840.c0 NOT GLOB 'abc') NOT IN (json_patch(s840.c1, s840.c2), 9223372036854775807)))) IS TRUE, ((SELECT max(s840.c0) FROM t0 AS s840 WHERE ((s840.c0 NOT GLOB 'abc') NOT IN (json_patch(s840.c1, s840.c2), 9223372036854775807)))) IS FALSE FROM t2 AS a0 WHERE a0.rowid = 11;
SELECT a0.rowid FROM t2 AS a0 WHERE (NOT (SELECT max(s840.c0) FROM t0 AS s840 WHERE ((s840.c0 NOT GLOB 'abc') NOT IN (json_patch(s840.c1, s840.c2), 9223372036854775807))));
```

## What differed

```
Amphora errors where SQLite finds the pivot: Parse error: Unexpected character
```
