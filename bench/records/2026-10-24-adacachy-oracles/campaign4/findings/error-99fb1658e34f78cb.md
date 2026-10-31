# pqs Error: Amphora errors where SQLite finds the pivot: integer overflow

seed 14, state 7840

## Statements

```sql
CREATE TABLE t0(c0 INT);
INSERT OR IGNORE INTO t0 VALUES (x'41'), (10), (9223372036854775807);
INSERT INTO t0 VALUES ('abc'), (-1);
INSERT OR IGNORE INTO t0 VALUES ('A'), (x'41');
INSERT OR IGNORE INTO t0 VALUES (7), (NULL);
INSERT OR IGNORE INTO t0 VALUES (2), (x'00'), ('a');
```

## Queries

```sql
SELECT ((1 OR (SELECT sum(s351.c0) FROM t0 AS s351 WHERE s351.c0))) IS TRUE, ((1 OR (SELECT sum(s351.c0) FROM t0 AS s351 WHERE s351.c0))) IS FALSE FROM t0 AS a0 WHERE a0.rowid = 12;
SELECT a0.rowid FROM t0 AS a0 WHERE (1 OR (SELECT sum(s351.c0) FROM t0 AS s351 WHERE s351.c0));
```

## What differed

```
Amphora errors where SQLite finds the pivot: integer overflow
```
