# pqs Error: Amphora errors where SQLite finds the pivot: json_set() needs an odd number of arguments

seed 27004, state 8407

## Statements

```sql
CREATE TABLE t1(c0 BLOB, c1 TEXT PRIMARY KEY, c2 INT COLLATE NOCASE) STRICT;
CREATE TABLE t2(c0 BOOLEAN COLLATE BINARY, c1 NUMERIC PRIMARY KEY, c2 NUMERIC COLLATE RTRIM);
INSERT OR IGNORE INTO t1 VALUES (x'0102', '', 7);
```

## Queries

```sql
SELECT ((SELECT min(s455.c1) FROM t2 AS s455 WHERE (atanh(a0.c1) BETWEEN jsonb_set(a0.c1, 1.5) AND (s455.c0 COLLATE NOCASE)))) IS TRUE, ((SELECT min(s455.c1) FROM t2 AS s455 WHERE (atanh(a0.c1) BETWEEN jsonb_set(a0.c1, 1.5) AND (s455.c0 COLLATE NOCASE)))) IS FALSE FROM t1 AS a0 WHERE a0.rowid = 1;
SELECT a0.rowid FROM t1 AS a0 WHERE ((SELECT min(s455.c1) FROM t2 AS s455 WHERE (atanh(a0.c1) BETWEEN jsonb_set(a0.c1, 1.5) AND (s455.c0 COLLATE NOCASE))) ISNULL);
```

## What differed

```
Amphora errors where SQLite finds the pivot: json_set() needs an odd number of arguments
```
