# pqs Error: Amphora errors where SQLite finds the pivot: FLAGS parameter to json_valid() must be between 1 and 15

seed 20261028, state 66972

## Statements

```sql
CREATE TABLE t0(c0 NUMERIC PRIMARY KEY NOT NULL, c1 NUMERIC COLLATE NOCASE);
INSERT OR IGNORE INTO t0 VALUES (x'c3bf', 9223372036854775807);
```

## Queries

```sql
SELECT (((CASE WHEN (SELECT sum(s577.c0) FROM t0 AS s577 WHERE (s577.c1 ISNULL)) THEN -100 ELSE json_valid(NULL, NULL) END) IN (NULL))) IS TRUE, (((CASE WHEN (SELECT sum(s577.c0) FROM t0 AS s577 WHERE (s577.c1 ISNULL)) THEN -100 ELSE json_valid(NULL, NULL) END) IN (NULL))) IS FALSE FROM t0 AS a0 WHERE a0.rowid = 1;
SELECT a0.rowid FROM t0 AS a0 WHERE (((CASE WHEN (SELECT sum(s577.c0) FROM t0 AS s577 WHERE (s577.c1 ISNULL)) THEN -100 ELSE json_valid(NULL, NULL) END) IN (NULL)) ISNULL);
```

## What differed

```
Amphora errors where SQLite finds the pivot: FLAGS parameter to json_valid() must be between 1 and 15
```
