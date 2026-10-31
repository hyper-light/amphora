# pqs Error: Amphora errors where SQLite finds the pivot: JSON cannot hold BLOB values

seed 14, state 18922

## Statements

```sql
CREATE TABLE t0(c0 INTEGER, c1  COLLATE RTRIM, c2 VARCHAR(10) UNIQUE, c3 BOOLEAN UNIQUE);
INSERT OR IGNORE INTO t0 VALUES (-1e-308, 10, NULL, 0.1);
INSERT OR IGNORE INTO t0 VALUES (NULL, '%', x'', 1.5);
```

## Queries

```sql
SELECT ((SELECT min(s101.c3) FROM t0 AS s101 WHERE (json_array(s101.c1, s101.c2) != (CASE WHEN (s101.c3 == 2.0) THEN '' WHEN (NOT (s101.c2 GLOB '%')) THEN '_' ELSE s101.c0 END)))) IS TRUE, ((SELECT min(s101.c3) FROM t0 AS s101 WHERE (json_array(s101.c1, s101.c2) != (CASE WHEN (s101.c3 == 2.0) THEN '' WHEN (NOT (s101.c2 GLOB '%')) THEN '_' ELSE s101.c0 END)))) IS FALSE FROM t0 AS a0 WHERE a0.rowid = 2;
SELECT a0.rowid FROM t0 AS a0 WHERE (SELECT min(s101.c3) FROM t0 AS s101 WHERE (json_array(s101.c1, s101.c2) != (CASE WHEN (s101.c3 == 2.0) THEN '' WHEN (NOT (s101.c2 GLOB '%')) THEN '_' ELSE s101.c0 END)));
```

## What differed

```
Amphora errors where SQLite finds the pivot: JSON cannot hold BLOB values
```
