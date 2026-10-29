# pqs Error: Amphora errors where SQLite finds the pivot: json_object() labels must be TEXT

seed 20261028, state 5854

## Statements

```sql
CREATE TABLE t0(c0 DOUBLE COLLATE BINARY NOT NULL, c1 INT UNIQUE, c2 INTEGER COLLATE BINARY UNIQUE NOT NULL, c3 VARCHAR(10) COLLATE NOCASE);
CREATE TABLE t1(c0 TEXT COLLATE BINARY, c1 BOOLEAN, c2 TEXT COLLATE BINARY);
INSERT INTO t1 VALUES (255, '0', 'a '), (1e308, 3.0, x'c3bf'), (x'00', NULL, 'true');
```

## Queries

```sql
SELECT ((NOT EXISTS (SELECT 1 FROM t0 AS s15 WHERE (date(a0.c2) BETWEEN substr(x'', s15.c0) AND json_object(255, 'a'))))) IS TRUE, ((NOT EXISTS (SELECT 1 FROM t0 AS s15 WHERE (date(a0.c2) BETWEEN substr(x'', s15.c0) AND json_object(255, 'a'))))) IS FALSE FROM t1 AS a0 WHERE a0.rowid = 3;
SELECT a0.rowid FROM t1 AS a0 WHERE (NOT EXISTS (SELECT 1 FROM t0 AS s15 WHERE (date(a0.c2) BETWEEN substr(x'', s15.c0) AND json_object(255, 'a'))));
```

## What differed

```
Amphora errors where SQLite finds the pivot: json_object() labels must be TEXT
```
