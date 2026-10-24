# pqs Error: Amphora errors where SQLite finds the pivot: integer overflow

seed 12, state 71

## Statements

```sql
CREATE TABLE t1(c0 BLOB, c1 BOOLEAN, c2 DECIMAL(10,2), c3 );
INSERT INTO t1 VALUES (' ', 'ABC', 'a ', -100);
CREATE TABLE t2(c0 , c1 NUMERIC, c2 REAL NOT NULL, c3 VARCHAR(10) COLLATE BINARY NOT NULL);
INSERT INTO t2 VALUES (3, '1e3', 2.0, 10), ('-1', 10, '1e3', 9223372036854775807);
INSERT INTO t2 VALUES ('ABC', '10', -1e-308, 'ABC');
```

## Queries

```sql
SELECT ((SELECT sum(s255.c3) FROM t2 AS s255 WHERE (((s255.c3 < s255.c2) OR (a0.c3 NOTNULL)) OR (s255.c3 NOT BETWEEN a0.c3 AND -9223372036854775808)))) IS TRUE, ((SELECT sum(s255.c3) FROM t2 AS s255 WHERE (((s255.c3 < s255.c2) OR (a0.c3 NOTNULL)) OR (s255.c3 NOT BETWEEN a0.c3 AND -9223372036854775808)))) IS FALSE FROM t1 AS a0 WHERE a0.rowid = 1;
SELECT a0.rowid FROM t1 AS a0 WHERE (SELECT sum(s255.c3) FROM t2 AS s255 WHERE (((s255.c3 < s255.c2) OR (a0.c3 NOTNULL)) OR (s255.c3 NOT BETWEEN a0.c3 AND -9223372036854775808)));
```

## What differed

```
Amphora errors where SQLite finds the pivot: integer overflow
```
