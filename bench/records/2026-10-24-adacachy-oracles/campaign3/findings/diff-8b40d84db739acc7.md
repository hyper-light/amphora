# diff Wrong: Amphora's answer isn't SQLite's

seed 13, state 1885

## Statements

```sql
CREATE TABLE t1(c0 NUMERIC PRIMARY KEY);
INSERT OR IGNORE INTO t1 VALUES (x'00'), (1.5);
```

## Queries

```sql
SELECT '{"a":1,"b":[2,3]}' FROM t1 AS a0 RIGHT JOIN t1 AS a1 ON ((0 || a1.c0) BETWEEN 0 AND a0.c0);
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 4 rows
    '{"a":1,"b":[2,3]}'
    '{"a":1,"b":[2,3]}'
    '{"a":1,"b":[2,3]}'
    '{"a":1,"b":[2,3]}'
  Amphora:  3 rows
    '{"a":1,"b":[2,3]}'
    '{"a":1,"b":[2,3]}'
    '{"a":1,"b":[2,3]}'
```
