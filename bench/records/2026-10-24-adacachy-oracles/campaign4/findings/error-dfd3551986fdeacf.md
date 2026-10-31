# pqs Error: Amphora errors where SQLite finds the pivot: json_object() requires an even number of arguments

seed 14, state 9404

## Statements

```sql
CREATE TABLE t0(c0 , c1 INT COLLATE RTRIM);
INSERT OR IGNORE INTO t0 VALUES (1, 0.0), (-1, 1), (NULL, '0x10');
INSERT OR IGNORE INTO t0 VALUES (100, x'41'), (-1, 7), (NULL, -1);
```

## Queries

```sql
SELECT ((jsonb_object(NULL, 0.0, NULL) OR ((- 1.5) NOTNULL))) IS TRUE, ((jsonb_object(NULL, 0.0, NULL) OR ((- 1.5) NOTNULL))) IS FALSE FROM t0 AS a0 WHERE a0.rowid = 6;
SELECT a0.rowid FROM t0 AS a0 WHERE (jsonb_object(NULL, 0.0, NULL) OR ((- 1.5) NOTNULL));
```

## What differed

```
Amphora errors where SQLite finds the pivot: json_object() requires an even number of arguments
```
