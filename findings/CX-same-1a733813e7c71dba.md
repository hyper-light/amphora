# same Wrong: the statements leave other rows to SELECT * FROM lg

seed 8004, state 13616

## Statements

```sql
CREATE TABLE t0(c0 INTEGER COLLATE NOCASE PRIMARY KEY, c1 BLOB) WITHOUT ROWID;
CREATE TABLE lg(c0, c1, c2);
CREATE TRIGGER g1 BEFORE INSERT ON t0 FOR EACH ROW BEGIN INSERT INTO lg VALUES ('g1', (NOT ('a' IS new.c0)), (2 BETWEEN new.c1 AND new.c0)); END;
INSERT INTO t0 VALUES ('A', -1e-308);
```

## Queries

```sql
SELECT * FROM lg;
```

## What differed

```
the statements leave other rows to SELECT * FROM lg
  expected: 1 rows
    'g1', 0, 1
  Amphora:  1 rows
    'g1', 1, 1
```
