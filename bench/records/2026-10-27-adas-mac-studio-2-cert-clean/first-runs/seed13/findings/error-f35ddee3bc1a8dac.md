# pqs Error: Amphora errors where SQLite finds the pivot: json_insert() needs an odd number of arguments

seed 13, state 15070

## Statements

```sql
CREATE TABLE t0(c0 DOUBLE PRIMARY KEY, c1 INTEGER NOT NULL, c2 BLOB COLLATE BINARY);
INSERT OR IGNORE INTO t0 VALUES (-1, 10, 0.0);
```

## Queries

```sql
SELECT ((NOT EXISTS (SELECT 1 FROM t0 AS s295 WHERE (s295.c0 BETWEEN jsonb_insert(s295.c1, '1') AND json_extract('"x"', '$[0]'))))) IS TRUE, ((NOT EXISTS (SELECT 1 FROM t0 AS s295 WHERE (s295.c0 BETWEEN jsonb_insert(s295.c1, '1') AND json_extract('"x"', '$[0]'))))) IS FALSE FROM t0 AS a0 WHERE a0.rowid = 1;
SELECT a0.rowid FROM t0 AS a0 WHERE (NOT EXISTS (SELECT 1 FROM t0 AS s295 WHERE (s295.c0 BETWEEN jsonb_insert(s295.c1, '1') AND json_extract('"x"', '$[0]'))));
```

## What differed

```
Amphora errors where SQLite finds the pivot: json_insert() needs an odd number of arguments
```
