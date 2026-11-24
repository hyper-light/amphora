# same Wrong: the statements leave other rows to SELECT * FROM lg

seed 8004, state 25909

## Statements

```sql
CREATE TABLE t1(c0 REAL) STRICT;
CREATE TABLE lg(c0, c1, c2);
CREATE TRIGGER g0 BEFORE INSERT ON t1 BEGIN INSERT INTO lg VALUES ('g0', (new.c0 COLLATE NOCASE), new.c0); END;
INSERT OR IGNORE INTO t1 VALUES (3);
```

## Queries

```sql
SELECT * FROM lg;
```

## What differed

```
the statements leave other rows to SELECT * FROM lg
  expected: 1 rows
    'g0', 3.0, 3.0
  Amphora:  1 rows
    'g0', 3, 3
```
