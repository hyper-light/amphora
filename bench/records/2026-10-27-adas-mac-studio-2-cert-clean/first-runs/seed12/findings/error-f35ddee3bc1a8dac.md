# pqs Error: Amphora errors where SQLite finds the pivot: json_insert() needs an odd number of arguments

seed 12, state 27488

## Statements

```sql
CREATE TABLE t0(c0 NUMERIC PRIMARY KEY);
INSERT INTO t0 VALUES (NULL), (10);
INSERT OR IGNORE INTO t0 VALUES (1e308);
CREATE INDEX i1 ON t0(c0) WHERE c0 IS NOT NULL;
```

## Queries

```sql
SELECT ((('_' * a0.c0) IN (0, jsonb_insert(-9223372036854775808, NULL), a0.c0))) IS TRUE, ((('_' * a0.c0) IN (0, jsonb_insert(-9223372036854775808, NULL), a0.c0))) IS FALSE FROM t0 AS a0 WHERE a0.rowid = 3;
SELECT a0.rowid FROM t0 AS a0 WHERE (('_' * a0.c0) IN (0, jsonb_insert(-9223372036854775808, NULL), a0.c0));
```

## What differed

```
Amphora errors where SQLite finds the pivot: json_insert() needs an odd number of arguments
```
