# diff Wrong: Amphora's answer isn't SQLite's

seed 13, state 1859

## Statements

```sql
CREATE TABLE t1(c0 DECIMAL(10,2), c1 DOUBLE, c2 NUMERIC, c3 );
INSERT INTO t1 VALUES (NULL, 2, 0.5, '1.5');
```

## Queries

```sql
SELECT NULL FROM t1 AS a0 WHERE (NOT (CASE a0.c3 WHEN a0.c2 THEN NULL WHEN a0.c3 THEN 0 END));
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    NULL
  Amphora:  0 rows
```
