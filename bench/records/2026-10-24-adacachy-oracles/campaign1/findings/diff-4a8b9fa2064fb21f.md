# diff Wrong: Amphora's answer isn't SQLite's

seed 11, state 354

## Statements

```sql
CREATE TABLE t1(c0 NUMERIC UNIQUE);
CREATE TABLE t2(c0 DOUBLE, c1 , c2 , c3 BOOLEAN);
INSERT OR IGNORE INTO t2 VALUES (-9223372036854775808, -2.5, 1000000000000000.0, '_');
```

## Queries

```sql
SELECT quote(a1.c2) FROM t1 AS a0 RIGHT JOIN t2 AS a1;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    '1000000000000000.0'
  Amphora:  1 rows
    '1.0e+15'
```
