# pqs Error: Amphora errors where SQLite finds the pivot: JSON cannot hold BLOB values

seed 13, state 54116

## Statements

```sql
CREATE TABLE t0(c0 TEXT COLLATE NOCASE UNIQUE, c1 VARCHAR(10), c2 BOOLEAN PRIMARY KEY);
CREATE TABLE t1(c0 NUMERIC COLLATE RTRIM, c1 , c2 REAL);
INSERT OR IGNORE INTO t1 VALUES (0.1, -1e-308, 100);
INSERT INTO t1 VALUES (0.5, x'41', 100), ('true', 1000000000000000.0, NULL), ('a ', 100, -1);
INSERT OR IGNORE INTO t1 VALUES (NULL, ' ', NULL), (x'0102', x'0102', '1.5'), (2.0, 1, x'0102');
```

## Queries

```sql
SELECT (((~ 0) NOT BETWEEN (EXISTS (SELECT 1 FROM t0 AS s717 WHERE (0 NOT GLOB 'a*'))) AND json_quote(a0.c0))) IS TRUE, (((~ 0) NOT BETWEEN (EXISTS (SELECT 1 FROM t0 AS s717 WHERE (0 NOT GLOB 'a*'))) AND json_quote(a0.c0))) IS FALSE FROM t1 AS a0 WHERE a0.rowid = 7;
SELECT a0.rowid FROM t1 AS a0 WHERE ((~ 0) NOT BETWEEN (EXISTS (SELECT 1 FROM t0 AS s717 WHERE (0 NOT GLOB 'a*'))) AND json_quote(a0.c0));
```

## What differed

```
Amphora errors where SQLite finds the pivot: JSON cannot hold BLOB values
```
