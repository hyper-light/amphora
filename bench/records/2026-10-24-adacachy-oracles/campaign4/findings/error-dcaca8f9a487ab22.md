# pqs Error: Amphora errors where SQLite finds the pivot: json_array_insert() needs an odd number of arguments

seed 14, state 22746

## Statements

```sql
CREATE TABLE t0(c0 TEXT COLLATE RTRIM PRIMARY KEY);
INSERT INTO t0 VALUES (0.0), ('true');
INSERT INTO t0 VALUES (1), (9223372036854775807);
```

## Queries

```sql
SELECT ((jsonb_array_insert(NULL, NULL) AND ((+ x'') ISNULL))) IS TRUE, ((jsonb_array_insert(NULL, NULL) AND ((+ x'') ISNULL))) IS FALSE FROM t0 AS a0 WHERE a0.rowid = 4;
SELECT a0.rowid FROM t0 AS a0 WHERE (NOT (jsonb_array_insert(NULL, NULL) AND ((+ x'') ISNULL)));
```

## What differed

```
Amphora errors where SQLite finds the pivot: json_array_insert() needs an odd number of arguments
```
