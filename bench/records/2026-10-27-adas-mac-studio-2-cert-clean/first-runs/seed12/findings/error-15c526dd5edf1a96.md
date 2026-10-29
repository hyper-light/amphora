# pqs Error: Amphora errors where SQLite finds the pivot: JSON cannot hold BLOB values

seed 12, state 56179

## Statements

```sql
CREATE TABLE t2(c0 TEXT COLLATE NOCASE, c1 NUMERIC COLLATE RTRIM PRIMARY KEY, c2 BOOLEAN, c3 REAL COLLATE RTRIM);
INSERT INTO t2 VALUES (10, -2.5, '0', '1e3');
INSERT OR IGNORE INTO t2 VALUES ('1e3', 0.0, x'0102', -2.5);
```

## Queries

```sql
SELECT ((SELECT min(s417.c1) FROM t2 AS s417 WHERE (('10' IS NOT s417.c3) NOT BETWEEN json_array('0x10', 2, s417.c2) AND 3))) IS TRUE, ((SELECT min(s417.c1) FROM t2 AS s417 WHERE (('10' IS NOT s417.c3) NOT BETWEEN json_array('0x10', 2, s417.c2) AND 3))) IS FALSE FROM t2 AS a0 WHERE a0.rowid = 1;
SELECT a0.rowid FROM t2 AS a0 WHERE (SELECT min(s417.c1) FROM t2 AS s417 WHERE (('10' IS NOT s417.c3) NOT BETWEEN json_array('0x10', 2, s417.c2) AND 3));
```

## What differed

```
Amphora errors where SQLite finds the pivot: JSON cannot hold BLOB values
```
