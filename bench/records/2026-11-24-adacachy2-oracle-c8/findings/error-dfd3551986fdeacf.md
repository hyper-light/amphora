# pqs Error: Amphora errors where SQLite finds the pivot: json_object() requires an even number of arguments

seed 903, state 2070

## Statements

```sql
CREATE TABLE t0(c0 INT PRIMARY KEY, c1 DOUBLE);
CREATE TABLE t1(c0  COLLATE NOCASE NOT NULL, c1 INT COLLATE NOCASE, c2 INTEGER, c3 TEXT COLLATE NOCASE);
INSERT OR IGNORE INTO t1 VALUES ('1.5', 9007199254740993, 0.1, 255), (9223372036854775807, 0.1, 0.5, -100);
INSERT INTO t1 VALUES (-1e-308, NULL, NULL, x'00'), ('x', 10, x'', -9223372036854775808);
```

## Queries

```sql
SELECT ((SELECT min(s235.c0) FROM t0 AS s235 WHERE (jsonb_object(7, a0.c3, 255) BETWEEN (s235.c1 << '_') AND a0.c0))) IS TRUE, ((SELECT min(s235.c0) FROM t0 AS s235 WHERE (jsonb_object(7, a0.c3, 255) BETWEEN (s235.c1 << '_') AND a0.c0))) IS FALSE FROM t1 AS a0 WHERE a0.rowid = 4;
SELECT a0.rowid FROM t1 AS a0 WHERE ((SELECT min(s235.c0) FROM t0 AS s235 WHERE (jsonb_object(7, a0.c3, 255) BETWEEN (s235.c1 << '_') AND a0.c0)) ISNULL);
```

## What differed

```
Amphora errors where SQLite finds the pivot: json_object() requires an even number of arguments
```
