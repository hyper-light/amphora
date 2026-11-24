# same Wrong: the statements leave other rows to SELECT rowid, * FROM t0

seed 8003, state 16786

## Statements

```sql
CREATE TABLE t0(c0 INT PRIMARY KEY, c1 REAL COLLATE NOCASE, c2 , c3 TEXT COLLATE RTRIM);
CREATE TABLE lg(c0, c1, c2);
CREATE TRIGGER g0 BEFORE INSERT ON t0 BEGIN INSERT INTO lg VALUES ('g0', x'00', CAST(new.c3 AS TEXT)); SELECT RAISE(IGNORE) WHERE ('A' BETWEEN new.c3 AND new.c1); END;
INSERT OR IGNORE INTO t0 VALUES ('%', '_', NULL, 9007199254740993);
```

## Queries

```sql
SELECT rowid, * FROM t0;
```

## What differed

```
the statements leave other rows to SELECT rowid, * FROM t0
  expected: 1 rows
    1, '%', '_', NULL, '9007199254740993'
  Amphora:  0 rows
```
