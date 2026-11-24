# diff Wrong: Amphora's answer isn't SQLite's

seed 8002, state 3291

## Statements

```sql
CREATE TABLE t1(c0  NOT NULL);
INSERT INTO t1 VALUES ('1.5');
```

## Queries

```sql
SELECT (1 || atan(a0.c0)) FROM t1 AS a0;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    '10.98279372324732905'
  Amphora:  1 rows
    '10.982793723247329'
```
