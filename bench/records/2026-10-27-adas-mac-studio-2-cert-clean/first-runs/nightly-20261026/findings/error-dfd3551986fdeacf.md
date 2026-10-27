# pqs Error: Amphora errors where SQLite finds the pivot: json_object() requires an even number of arguments

seed 20261026, state 26879

## Statements

```sql
CREATE TABLE t1(c0  PRIMARY KEY);
INSERT INTO t1 VALUES (3), (NULL);
INSERT INTO t1 VALUES (x'c3bf'), (7), (x'0102');
```

## Queries

```sql
SELECT ((NULL NOT IN (SELECT s882.c0 FROM t1 AS s882 WHERE ((~ x'00') BETWEEN (s882.c0 & 100) AND json_object('1e3'))))) IS TRUE, ((NULL NOT IN (SELECT s882.c0 FROM t1 AS s882 WHERE ((~ x'00') BETWEEN (s882.c0 & 100) AND json_object('1e3'))))) IS FALSE FROM t1 AS a0 WHERE a0.rowid = 5;
SELECT a0.rowid FROM t1 AS a0 WHERE (NULL NOT IN (SELECT s882.c0 FROM t1 AS s882 WHERE ((~ x'00') BETWEEN (s882.c0 & 100) AND json_object('1e3'))));
```

## What differed

```
Amphora errors where SQLite finds the pivot: json_object() requires an even number of arguments
```
