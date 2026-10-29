# pqs Error: Amphora errors where SQLite finds the pivot: integer overflow

seed 13, state 44042

## Statements

```sql
CREATE TABLE t0(c0 INTEGER PRIMARY KEY, c1 INT, c2 DECIMAL(10,2) COLLATE NOCASE, c3 TEXT);
INSERT OR IGNORE INTO t0 VALUES ('-1', 7, 0, x'41'), (1000000000000000.0, '0x10', 'a', 'abc');
INSERT OR IGNORE INTO t0 VALUES (-9223372036854775808, 1.5, 9223372036854775807, 1);
CREATE INDEX i0 ON t0(c1 DESC);
```

## Queries

```sql
SELECT ((SELECT sum(s73.c0) FROM t0 AS s73 WHERE (-2.5 NOTNULL))) IS TRUE, ((SELECT sum(s73.c0) FROM t0 AS s73 WHERE (-2.5 NOTNULL))) IS FALSE FROM t0 AS a0 WHERE a0.rowid = 1000000000000000;
SELECT a0.rowid FROM t0 AS a0 WHERE (SELECT sum(s73.c0) FROM t0 AS s73 WHERE (-2.5 NOTNULL));
```

## What differed

```
Amphora errors where SQLite finds the pivot: integer overflow
```
