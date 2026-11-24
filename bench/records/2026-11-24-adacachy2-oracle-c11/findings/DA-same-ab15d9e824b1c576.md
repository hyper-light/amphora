# same Wrong: the statements leave other rows to SELECT rowid, * FROM t1

seed 8002, state 2686

## Statements

```sql
CREATE TABLE t1(c0 INTEGER PRIMARY KEY, c1 TEXT) STRICT;
INSERT INTO t1 VALUES (2, '');
INSERT INTO t1 VALUES (2, '%') ON CONFLICT(c0) DO UPDATE SET c1 = excluded.c0;
```

## Queries

```sql
SELECT rowid, * FROM t1;
```

## What differed

```
the statements leave other rows to SELECT rowid, * FROM t1
  expected: 1 rows
    2, 2, '2'
  Amphora:  1 rows
    2, 2, NULL
```
