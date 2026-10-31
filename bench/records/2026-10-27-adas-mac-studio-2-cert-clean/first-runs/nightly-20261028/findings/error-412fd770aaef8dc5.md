# pqs Error: Amphora errors where SQLite finds the pivot: Parse error: bad JSON path: '10'

seed 20261028, state 28132

## Statements

```sql
CREATE TABLE t0(c0 NUMERIC COLLATE NOCASE UNIQUE, c1 NUMERIC PRIMARY KEY, c2 NUMERIC COLLATE BINARY);
INSERT OR IGNORE INTO t0 VALUES (1, -1, 3.0);
```

## Queries

```sql
SELECT ((EXISTS (SELECT 1 FROM t0 AS s474 WHERE (substr(' ', a0.c2) BETWEEN (NULL | s474.c1) AND json_extract(a0.c1, '10'))))) IS TRUE, ((EXISTS (SELECT 1 FROM t0 AS s474 WHERE (substr(' ', a0.c2) BETWEEN (NULL | s474.c1) AND json_extract(a0.c1, '10'))))) IS FALSE FROM t0 AS a0 WHERE a0.rowid = 1;
SELECT a0.rowid FROM t0 AS a0 WHERE (NOT (EXISTS (SELECT 1 FROM t0 AS s474 WHERE (substr(' ', a0.c2) BETWEEN (NULL | s474.c1) AND json_extract(a0.c1, '10')))));
```

## What differed

```
Amphora errors where SQLite finds the pivot: Parse error: bad JSON path: '10'
```
