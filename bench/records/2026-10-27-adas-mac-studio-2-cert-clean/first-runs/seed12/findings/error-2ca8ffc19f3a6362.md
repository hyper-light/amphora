# pqs Error: Amphora errors where SQLite finds the pivot: Parse error: Unexpected character

seed 12, state 6715

## Statements

```sql
CREATE TABLE t2(c0 REAL PRIMARY KEY, c1 VARCHAR(10), c2 VARCHAR(10) COLLATE NOCASE NOT NULL);
INSERT OR IGNORE INTO t2 VALUES (NULL, x'c3bf', 1);
```

## Queries

```sql
SELECT ((EXISTS (SELECT 1 FROM t2 AS s437 WHERE (NULL BETWEEN s437.c2 AND jsonb_array_insert(x'c3bf', 2.0, -1))))) IS TRUE, ((EXISTS (SELECT 1 FROM t2 AS s437 WHERE (NULL BETWEEN s437.c2 AND jsonb_array_insert(x'c3bf', 2.0, -1))))) IS FALSE FROM t2 AS a0 WHERE a0.rowid = 1;
SELECT a0.rowid FROM t2 AS a0 WHERE (NOT (EXISTS (SELECT 1 FROM t2 AS s437 WHERE (NULL BETWEEN s437.c2 AND jsonb_array_insert(x'c3bf', 2.0, -1)))));
```

## What differed

```
Amphora errors where SQLite finds the pivot: Parse error: Unexpected character
```
