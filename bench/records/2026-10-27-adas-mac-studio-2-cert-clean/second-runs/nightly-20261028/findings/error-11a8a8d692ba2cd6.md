# pqs Error: Amphora errors where SQLite finds the pivot: Parse error: bad JSON path: 'A'

seed 20261028, state 38808

## Statements

```sql
CREATE TABLE t0(c0 REAL COLLATE NOCASE UNIQUE, c1 INT, c2 TEXT UNIQUE);
INSERT OR IGNORE INTO t0 VALUES (' ', 255, x'c3bf');
CREATE TABLE t1(c0 BLOB PRIMARY KEY);
```

## Queries

```sql
SELECT ((EXISTS (SELECT 1 FROM t1 AS s487 WHERE (jsonb_extract(1e308, 'A', -100) BETWEEN (CASE WHEN ('A' > s487.c0) THEN s487.c0 END) AND 0.5)))) IS TRUE, ((EXISTS (SELECT 1 FROM t1 AS s487 WHERE (jsonb_extract(1e308, 'A', -100) BETWEEN (CASE WHEN ('A' > s487.c0) THEN s487.c0 END) AND 0.5)))) IS FALSE FROM t0 AS a0 WHERE a0.rowid = 1;
SELECT a0.rowid FROM t0 AS a0 WHERE (NOT (EXISTS (SELECT 1 FROM t1 AS s487 WHERE (jsonb_extract(1e308, 'A', -100) BETWEEN (CASE WHEN ('A' > s487.c0) THEN s487.c0 END) AND 0.5))));
```

## What differed

```
Amphora errors where SQLite finds the pivot: Parse error: bad JSON path: 'A'
```
