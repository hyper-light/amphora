# same Wrong: the statements leave other rows to SELECT * FROM lg

seed 8006, state 26688

## Statements

```sql
CREATE TABLE t1(c0 REAL UNIQUE) STRICT;
CREATE TABLE lg(c0, c1, c2);
CREATE TRIGGER g1 BEFORE INSERT ON t1 FOR EACH ROW BEGIN INSERT INTO lg VALUES ('g1', new.c0, (new.c0 << new.c0)); END;
INSERT INTO t1 VALUES (10);
```

## Queries

```sql
SELECT * FROM lg;
```

## What differed

```
the statements leave other rows to SELECT * FROM lg
  expected: 1 rows
    'g1', 10.0, 10240
  Amphora:  1 rows
    'g1', 10, 10240
```
