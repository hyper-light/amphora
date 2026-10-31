# pqs Error: Amphora errors where SQLite finds the pivot: JSON cannot hold BLOB values

seed 13, state 2714

## Statements

```sql
CREATE TABLE t0(c0 BLOB UNIQUE NOT NULL, c1 VARCHAR(10) NOT NULL, c2 INTEGER);
INSERT OR IGNORE INTO t0 VALUES (x'00', '10', 255), (0.0, 100, 'A'), (7, -1e-308, 'A');
INSERT OR IGNORE INTO t0 VALUES ('-1', x'0102', 7);
INSERT OR IGNORE INTO t0 VALUES ('0x10', 1, 0.1), (9007199254740993, 10, 0.1);
INSERT OR IGNORE INTO t0 VALUES ('0', 1e308, 0), (x'c3bf', -2.5, NULL);
CREATE TABLE t1(c0 INTEGER COLLATE RTRIM, c1 , c2 INTEGER NOT NULL);
```

## Queries

```sql
SELECT (((json_object('1e3', a0.c1) IN (SELECT s12.c1 FROM t1 AS s12)) OR a0.c0)) IS TRUE, (((json_object('1e3', a0.c1) IN (SELECT s12.c1 FROM t1 AS s12)) OR a0.c0)) IS FALSE FROM t0 AS a0 WHERE a0.rowid = 8;
SELECT a0.rowid FROM t0 AS a0 WHERE (NOT ((json_object('1e3', a0.c1) IN (SELECT s12.c1 FROM t1 AS s12)) OR a0.c0));
```

## What differed

```
Amphora errors where SQLite finds the pivot: JSON cannot hold BLOB values
```
