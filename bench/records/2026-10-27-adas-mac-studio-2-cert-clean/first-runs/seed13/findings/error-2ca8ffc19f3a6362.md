# pqs Error: Amphora errors where SQLite finds the pivot: Parse error: Unexpected character

seed 13, state 1220

## Statements

```sql
CREATE TABLE t0(c0 DECIMAL(10,2) COLLATE NOCASE PRIMARY KEY NOT NULL, c1 TEXT COLLATE BINARY);
INSERT INTO t0 VALUES (1e308, -1);
INSERT INTO t0 VALUES (x'41', NULL);
CREATE TABLE t2(c0 BOOLEAN COLLATE NOCASE, c1 DOUBLE COLLATE NOCASE, c2 INT COLLATE BINARY PRIMARY KEY);
INSERT INTO t2 VALUES ('a', '', NULL);
CREATE UNIQUE INDEX i0 ON t0(c1 DESC);
```

## Queries

```sql
SELECT ((SELECT max(s797.c1) FROM t0 AS s797 WHERE (('_' / s797.c0) NOT BETWEEN (- s797.c1) AND json_pretty(s797.c0)))) IS TRUE, ((SELECT max(s797.c1) FROM t0 AS s797 WHERE (('_' / s797.c0) NOT BETWEEN (- s797.c1) AND json_pretty(s797.c0)))) IS FALSE FROM t2 AS a0 WHERE a0.rowid = 1;
SELECT a0.rowid FROM t2 AS a0 WHERE (SELECT max(s797.c1) FROM t0 AS s797 WHERE (('_' / s797.c0) NOT BETWEEN (- s797.c1) AND json_pretty(s797.c0)));
```

## What differed

```
Amphora errors where SQLite finds the pivot: Parse error: Unexpected character
```
