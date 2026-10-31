# diff Wrong: Amphora's answer isn't SQLite's

seed 11, state 965

## Statements

```sql
CREATE TABLE t0(c0  UNIQUE);
INSERT INTO t0 VALUES (x'');
```

## Queries

```sql
SELECT NULL FROM t0 AS a0 JOIN t0 AS a2 ON (a2.c0 NOT LIKE '%');
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 0 rows
  Amphora:  1 rows
    NULL
```
