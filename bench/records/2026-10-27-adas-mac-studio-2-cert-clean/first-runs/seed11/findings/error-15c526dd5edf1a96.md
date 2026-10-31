# pqs Error: Amphora errors where SQLite finds the pivot: JSON cannot hold BLOB values

seed 11, state 16340

## Statements

```sql
CREATE TABLE t0(c0 BLOB, c1 BLOB COLLATE BINARY, c2 DOUBLE);
INSERT OR IGNORE INTO t0 VALUES ('1e3', NULL, 10), (-1, 1.5, 3), (NULL, x'', '_');
```

## Queries

```sql
SELECT ((0 BETWEEN (NOT EXISTS (SELECT 1 FROM t0 AS s646 WHERE (3.0 IS NOT DISTINCT FROM 9223372036854775807))) AND json_quote(a0.c1))) IS TRUE, ((0 BETWEEN (NOT EXISTS (SELECT 1 FROM t0 AS s646 WHERE (3.0 IS NOT DISTINCT FROM 9223372036854775807))) AND json_quote(a0.c1))) IS FALSE FROM t0 AS a0 WHERE a0.rowid = 2;
SELECT a0.rowid FROM t0 AS a0 WHERE (NOT (0 BETWEEN (NOT EXISTS (SELECT 1 FROM t0 AS s646 WHERE (3.0 IS NOT DISTINCT FROM 9223372036854775807))) AND json_quote(a0.c1)));
```

## What differed

```
Amphora errors where SQLite finds the pivot: JSON cannot hold BLOB values
```
