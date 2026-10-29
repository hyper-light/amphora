# pqs Error: Amphora errors where SQLite finds the pivot: json_object() labels must be TEXT

seed 20261027, state 77013

## Statements

```sql
CREATE TABLE t1(c0 VARCHAR(10) COLLATE BINARY UNIQUE);
INSERT INTO t1 VALUES (2.0);
INSERT INTO t1 VALUES (1000000000000000.0), (3);
INSERT OR IGNORE INTO t1 VALUES (255), ('a '), (NULL);
INSERT OR IGNORE INTO t1 VALUES (NULL);
INSERT OR IGNORE INTO t1 VALUES (2), ('x'), ('-1');
CREATE INDEX i1 ON t1(c0 DESC) WHERE c0 IS NOT NULL;
```

## Queries

```sql
SELECT ((a0.c0 = jsonb_object(a0.c0, NULL))) IS TRUE, ((a0.c0 = jsonb_object(a0.c0, NULL))) IS FALSE FROM t1 AS a0 WHERE a0.rowid = 10;
SELECT a0.rowid FROM t1 AS a0 WHERE (NOT (a0.c0 = jsonb_object(a0.c0, NULL)));
```

## What differed

```
Amphora errors where SQLite finds the pivot: json_object() labels must be TEXT
```
