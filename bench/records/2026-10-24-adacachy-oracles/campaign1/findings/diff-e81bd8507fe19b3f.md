# diff Wrong: Amphora's answer isn't SQLite's

seed 11, state 594

## Statements

```sql
CREATE TABLE t0(c0 INTEGER, c1 NUMERIC COLLATE RTRIM, c2 BLOB UNIQUE, c3 VARCHAR(10));
INSERT INTO t0 VALUES (NULL, '10', 'é', x'41');
```

## Queries

```sql
SELECT NULL FROM t0 AS a0 JOIN t0 AS a2 ON (a2.c3 NOT LIKE '%a');
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 0 rows
  Amphora:  1 rows
    NULL
```
