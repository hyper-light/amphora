# diff Wrong: Amphora's answer isn't SQLite's

seed 14, state 11291

## Statements

```sql
CREATE TABLE t0(c0 DOUBLE, c1  COLLATE RTRIM PRIMARY KEY);
INSERT OR IGNORE INTO t0 VALUES (' ', NULL);
```

## Queries

```sql
SELECT a0.c1 FROM t0 AS a0, t0 AS a1 WHERE (char('1e3', (9007199254740993 COLLATE RTRIM), (NOT a0.c1)) < a1.c0);
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 0 rows
  Amphora:  1 rows
    NULL
```
