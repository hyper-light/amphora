# pqs Error: Amphora errors where SQLite finds the pivot: string or blob too big

seed 13, state 12664

## Statements

```sql
CREATE TABLE t0(c0 VARCHAR(10) PRIMARY KEY, c1 REAL COLLATE BINARY);
CREATE TABLE t1(c0 NUMERIC UNIQUE);
INSERT INTO t1 VALUES (NULL);
INSERT OR IGNORE INTO t1 VALUES (NULL), (NULL);
INSERT INTO t1 VALUES (1000000000000000.0);
INSERT OR IGNORE INTO t1 VALUES ('-1'), (x'c3bf');
```

## Queries

```sql
SELECT ((SELECT sum(s537.c1) FROM t0 AS s537 WHERE ((100 BETWEEN x'' AND 'ABC') BETWEEN substring(s537.c0, s537.c1) AND zeroblob(a0.c0)))) IS TRUE, ((SELECT sum(s537.c1) FROM t0 AS s537 WHERE ((100 BETWEEN x'' AND 'ABC') BETWEEN substring(s537.c0, s537.c1) AND zeroblob(a0.c0)))) IS FALSE FROM t1 AS a0 WHERE a0.rowid = 6;
SELECT a0.rowid FROM t1 AS a0 WHERE ((SELECT sum(s537.c1) FROM t0 AS s537 WHERE ((100 BETWEEN x'' AND 'ABC') BETWEEN substring(s537.c0, s537.c1) AND zeroblob(a0.c0))) ISNULL);
```

## What differed

```
Amphora errors where SQLite finds the pivot: string or blob too big
```
