# same Wrong: the statements leave other rows to SELECT rowid, * FROM t0

seed 8003, state 6294

## Statements

```sql
CREATE TABLE t0(c0 INTEGER PRIMARY KEY, c1 INT) STRICT;
INSERT INTO t0 VALUES (10, 1);
INSERT INTO t0 VALUES (10, 0) ON CONFLICT(c0) DO UPDATE SET c0 = excluded.c1, c1 = excluded.c0;
```

## Queries

```sql
SELECT rowid, * FROM t0;
```

## What differed

```
the statements leave other rows to SELECT rowid, * FROM t0
  expected: 1 rows
    0, 0, 10
  Amphora:  1 rows
    0, 0, NULL
```
