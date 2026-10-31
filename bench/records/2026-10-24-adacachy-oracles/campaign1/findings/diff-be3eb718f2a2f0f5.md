# diff Wrong: Amphora's answer isn't SQLite's

seed 11, state 688

## Statements

```sql
CREATE TABLE t0(c0 DOUBLE PRIMARY KEY);
CREATE TABLE t1(c0  PRIMARY KEY);
INSERT OR IGNORE INTO t1 VALUES (x'41');
```

## Queries

```sql
SELECT NULL FROM t0 AS a0 RIGHT JOIN t1 AS a1 ON NULL FULL JOIN t1 AS a2 ON (a2.c0 NOT LIKE 'A%');
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 2 rows
    NULL
    NULL
  Amphora:  1 rows
    NULL
```
