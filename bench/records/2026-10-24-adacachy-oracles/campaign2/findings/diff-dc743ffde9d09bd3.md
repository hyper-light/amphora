# diff Wrong: Amphora's answer isn't SQLite's

seed 12, state 3639

## Statements

```sql
CREATE TABLE t0(c0 BOOLEAN, c1 BLOB UNIQUE, c2 REAL, c3 NUMERIC COLLATE RTRIM);
INSERT INTO t0 VALUES (0.1, 7, 'a ', '%');
CREATE TABLE t2(c0 REAL);
INSERT INTO t2 VALUES ('x');
```

## Queries

```sql
SELECT NULL FROM t0 AS a0 FULL JOIN t2 AS a2 ON (json_extract('{"a":1,"b":[2,3]}', '$.a') BETWEEN a0.c0 AND 2);
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    NULL
  Amphora:  2 rows
    NULL
    NULL
```
