# pqs Wrong: the pivot (rowid 2), whose predicate SQLite rectified to TRUE, is missing

seed 11, state 665

## Statements

```sql
CREATE TABLE t1(c0 INT, c1 DOUBLE COLLATE NOCASE PRIMARY KEY, c2 REAL);
INSERT INTO t1 VALUES (x'00', 255, -100), (0, 'A', '0x10');
```

## Queries

```sql
SELECT ((jsonb_patch(0, 0) GLOB '*')) IS TRUE, ((jsonb_patch(0, 0) GLOB '*')) IS FALSE FROM t1 AS a0 WHERE a0.rowid = 2;
SELECT a0.rowid FROM t1 AS a0 WHERE (jsonb_patch(0, 0) GLOB '*');
```

## What differed

```
the pivot (rowid 2), whose predicate SQLite rectified to TRUE, is missing
  Amphora: 0 rows
```
