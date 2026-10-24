# pqs Error: Amphora errors where SQLite finds the pivot: JSON path error near: "0"

seed 12, state 1302

## Statements

```sql
CREATE TABLE t2(c0 VARCHAR(10) PRIMARY KEY);
INSERT OR IGNORE INTO t2 VALUES (255);
INSERT OR IGNORE INTO t2 VALUES (0.0), (2);
INSERT OR IGNORE INTO t2 VALUES (x'0102');
INSERT INTO t2 VALUES (NULL), (10);
INSERT OR IGNORE INTO t2 VALUES (3.0), (NULL), (1);
INSERT OR IGNORE INTO t2 VALUES (''), (NULL), ('a ');
```

## Queries

```sql
SELECT ((json_array_length(10, 0) OR ('1.5' NOTNULL))) IS TRUE, ((json_array_length(10, 0) OR ('1.5' NOTNULL))) IS FALSE FROM t2 AS a0 WHERE a0.rowid = 12;
SELECT a0.rowid FROM t2 AS a0 WHERE (json_array_length(10, 0) OR ('1.5' NOTNULL));
```

## What differed

```
Amphora errors where SQLite finds the pivot: JSON path error near: "0"
```
