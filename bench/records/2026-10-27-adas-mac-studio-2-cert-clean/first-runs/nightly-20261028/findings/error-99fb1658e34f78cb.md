# pqs Error: Amphora errors where SQLite finds the pivot: integer overflow

seed 20261028, state 21557

## Statements

```sql
CREATE TABLE t0(c0 INTEGER COLLATE BINARY PRIMARY KEY, c1 DECIMAL(10,2), c2 DECIMAL(10,2) UNIQUE, c3 NUMERIC);
INSERT INTO t0 VALUES (-9223372036854775808, x'0102', -100, '1.5');
INSERT OR IGNORE INTO t0 VALUES (-1, x'41', 'a', 'x'), (3.0, 3.0, 9007199254740993, 3);
```

## Queries

```sql
SELECT ((SELECT sum(s448.c0) FROM t0 AS s448 WHERE (-1e-308 IS NOT s448.c0))) IS TRUE, ((SELECT sum(s448.c0) FROM t0 AS s448 WHERE (-1e-308 IS NOT s448.c0))) IS FALSE FROM t0 AS a0 WHERE a0.rowid = 3;
SELECT a0.rowid FROM t0 AS a0 WHERE (SELECT sum(s448.c0) FROM t0 AS s448 WHERE (-1e-308 IS NOT s448.c0));
```

## What differed

```
Amphora errors where SQLite finds the pivot: integer overflow
```
