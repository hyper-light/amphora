# pqs Error: Amphora errors where SQLite finds the pivot: json_object() labels must be TEXT

seed 13, state 7337

## Statements

```sql
CREATE TABLE t0(c0 BLOB);
INSERT INTO t0 VALUES (0.1), ('_');
INSERT OR IGNORE INTO t0 VALUES (x'41'), (2.0), ('é');
```

## Queries

```sql
SELECT ((json_object(NULL, NULL) OR (1 OR 0.5))) IS TRUE, ((json_object(NULL, NULL) OR (1 OR 0.5))) IS FALSE FROM t0 AS a0 WHERE a0.rowid = 5;
SELECT a0.rowid FROM t0 AS a0 WHERE (json_object(NULL, NULL) OR (1 OR 0.5));
```

## What differed

```
Amphora errors where SQLite finds the pivot: json_object() labels must be TEXT
```
