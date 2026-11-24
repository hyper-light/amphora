# diff Wrong: Amphora's answer isn't SQLite's

seed 8002, state 358

## Statements

```sql
CREATE TABLE t0(c0 DOUBLE COLLATE BINARY);
INSERT INTO t0 VALUES (-1);
```

## Queries

```sql
SELECT NULL FROM t0 AS a0 WHERE ((NULL, (0 == a0.c0)) NOT IN (SELECT s878.c0, s878.c0 FROM t0 AS s878));
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    NULL
  Amphora:  0 rows
```
