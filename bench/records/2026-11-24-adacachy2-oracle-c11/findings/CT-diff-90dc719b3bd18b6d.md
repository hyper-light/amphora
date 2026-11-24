# diff Wrong: Amphora's answer isn't SQLite's

seed 8004, state 3436

## Statements

```sql
CREATE TABLE t2(c0 VARCHAR(10), c1 INT);
INSERT OR REPLACE INTO t2 VALUES (x'c3bf', x'00');
```

## Queries

```sql
SELECT NULL FROM t2 AS a0 WHERE ((NULL, a0.c1) NOT IN (SELECT s332.c1, s332.c0 FROM t2 AS s332));
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    NULL
  Amphora:  0 rows
```
