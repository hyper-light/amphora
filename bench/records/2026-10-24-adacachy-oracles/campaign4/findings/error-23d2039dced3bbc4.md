# pqs Error: Amphora errors where SQLite finds the pivot: FLAGS parameter to json_valid() must be between 1 and 15

seed 14, state 12497

## Statements

```sql
CREATE TABLE t1(c0 REAL, c1 INT, c2 );
INSERT OR IGNORE INTO t1 VALUES (7, 'abc', 3.0), (x'41', NULL, 'ABC'), (1000000000000000.0, 2.0, 10);
INSERT INTO t1 VALUES (x'0102', 1e308, 2.0), ('%', '', 0.0);
```

## Queries

```sql
SELECT ((NULL IN (SELECT s119.c0 FROM t1 AS s119 WHERE ((NULL >> NULL) BETWEEN replace(NULL, x'41', s119.c2) AND json_valid('true', a0.c0))))) IS TRUE, ((NULL IN (SELECT s119.c0 FROM t1 AS s119 WHERE ((NULL >> NULL) BETWEEN replace(NULL, x'41', s119.c2) AND json_valid('true', a0.c0))))) IS FALSE FROM t1 AS a0 WHERE a0.rowid = 5;
SELECT a0.rowid FROM t1 AS a0 WHERE (NOT (NULL IN (SELECT s119.c0 FROM t1 AS s119 WHERE ((NULL >> NULL) BETWEEN replace(NULL, x'41', s119.c2) AND json_valid('true', a0.c0)))));
```

## What differed

```
Amphora errors where SQLite finds the pivot: FLAGS parameter to json_valid() must be between 1 and 15
```
