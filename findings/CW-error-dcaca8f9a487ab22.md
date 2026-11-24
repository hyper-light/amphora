# pqs Error: Amphora errors where SQLite finds the pivot: json_array_insert() needs an odd number of arguments

seed 27006, state 4899

## Statements

```sql
CREATE TABLE t0(c0 INTEGER PRIMARY KEY NOT NULL) STRICT;
INSERT INTO t0 VALUES (0);
CREATE VIEW v0 AS SELECT DISTINCT CAST(((3.0 >= b90x0.c0) OR (b90x1.c0 <= -9223372036854775808)) AS INTEGER) AS c0, CAST(json_array_insert((b90x0.c0 | ' a'), b90x0.c0) AS INTEGER) AS c1, CAST(b90x0.c0 AS INTEGER) AS c2 FROM t0 AS b90x0 RIGHT JOIN t0 AS b90x1 ON (-100 IS CAST(b90x0.c0 AS BLOB));
```

## Queries

```sql
SELECT ((NULL IN (SELECT s540.c1 FROM v0 AS s540 WHERE (s540.c2 IN (s540.c1))))) IS TRUE, ((NULL IN (SELECT s540.c1 FROM v0 AS s540 WHERE (s540.c2 IN (s540.c1))))) IS FALSE FROM t0 AS a0 WHERE a0.rowid = 0;
SELECT a0.rowid FROM t0 AS a0 WHERE (NOT (NULL IN (SELECT s540.c1 FROM v0 AS s540 WHERE (s540.c2 IN (s540.c1)))));
```

## What differed

```
Amphora errors where SQLite finds the pivot: json_array_insert() needs an odd number of arguments
```
