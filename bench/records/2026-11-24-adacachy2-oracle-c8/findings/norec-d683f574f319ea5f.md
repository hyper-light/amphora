# norec Panic: indexbtree_seek_unpacked: index_info required

seed 1012, state 6238

## Statements

```sql
CREATE TABLE t0(c0 BLOB, c1 TEXT);
CREATE TABLE t1(c0 INTEGER);
INSERT INTO t1 VALUES ('10');
CREATE TABLE t2(c0 REAL, c1 DECIMAL(10,2) COLLATE NOCASE, c2 TEXT UNIQUE);
INSERT OR IGNORE INTO t2 VALUES (2.0, 9007199254740993, x'41');
```

## Queries

```sql
SELECT -100 FROM t2 AS a0, t0 AS a1 FULL JOIN t2 AS a2 ON (a0.c0 NOT IN (SELECT s995.c1 FROM t0 AS s995 WHERE (NULL NOT IN (s995.c0, s995.c1))));
SELECT -100 FROM t2 AS a0, t0 AS a1 FULL JOIN t2 AS a2 ON (a0.c0 NOT IN (SELECT s995.c1 FROM t0 AS s995 WHERE (NULL NOT IN (s995.c0, s995.c1)))) WHERE ((sign(a1.c0) | (a1.c1 = 9007199254740993)) IN (SELECT s117.c0 FROM t1 AS s117 WHERE ((s117.c0 IN ('true', s117.c0, -100)) IN (SELECT s527.c1 FROM t0 AS s527 WHERE (s527.c1 == a1.c1)))));
SELECT -100 FROM t2 AS a0, t0 AS a1 FULL JOIN t2 AS a2 ON (a0.c0 NOT IN (SELECT s995.c1 FROM t0 AS s995 WHERE (NULL NOT IN (s995.c0, s995.c1)))) WHERE (NOT ((sign(a1.c0) | (a1.c1 = 9007199254740993)) IN (SELECT s117.c0 FROM t1 AS s117 WHERE ((s117.c0 IN ('true', s117.c0, -100)) IN (SELECT s527.c1 FROM t0 AS s527 WHERE (s527.c1 == a1.c1))))));
SELECT -100 FROM t2 AS a0, t0 AS a1 FULL JOIN t2 AS a2 ON (a0.c0 NOT IN (SELECT s995.c1 FROM t0 AS s995 WHERE (NULL NOT IN (s995.c0, s995.c1)))) WHERE (((sign(a1.c0) | (a1.c1 = 9007199254740993)) IN (SELECT s117.c0 FROM t1 AS s117 WHERE ((s117.c0 IN ('true', s117.c0, -100)) IN (SELECT s527.c1 FROM t0 AS s527 WHERE (s527.c1 == a1.c1))))) ISNULL);
```

## What differed

```
indexbtree_seek_unpacked: index_info required
```
