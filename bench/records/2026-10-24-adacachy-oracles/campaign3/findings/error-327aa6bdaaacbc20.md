# pqs Error: Amphora errors where SQLite finds the pivot: JSON path error near: "7"

seed 13, state 3036

## Statements

```sql
CREATE TABLE t0(c0 REAL, c1 BLOB, c2 INT PRIMARY KEY NOT NULL);
INSERT OR IGNORE INTO t0 VALUES ('1', 'abc', x'0102');
INSERT OR IGNORE INTO t0 VALUES ('1', 1e308, x'41');
INSERT INTO t0 VALUES (NULL, NULL, '-1');
INSERT INTO t0 VALUES (NULL, 3.0, 0.0);
```

## Queries

```sql
SELECT ((jsonb_replace(100, 7, NULL) AND (NULL AND 0))) IS TRUE, ((jsonb_replace(100, 7, NULL) AND (NULL AND 0))) IS FALSE FROM t0 AS a0 WHERE a0.rowid = 4;
SELECT a0.rowid FROM t0 AS a0 WHERE (NOT (jsonb_replace(100, 7, NULL) AND (NULL AND 0)));
```

## What differed

```
Amphora errors where SQLite finds the pivot: JSON path error near: "7"
```
