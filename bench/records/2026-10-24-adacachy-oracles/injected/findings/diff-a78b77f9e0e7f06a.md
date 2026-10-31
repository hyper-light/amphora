# diff Wrong: Amphora's answer isn't SQLite's

seed 20261024, state 52

## Statements

```sql
CREATE TABLE t0(c0  COLLATE RTRIM, c1 TEXT, c2 VARCHAR(10));
INSERT INTO t0 VALUES (x'0102', 0, x'ff');
```

## Queries

```sql
SELECT '{"a":1,"b":[2,3]}' FROM t0 AS a0 LEFT JOIN t0 AS a1 ON NULL WHERE (a1.c1 ISNULL);
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    '{"a":1,"b":[2,3]}'
  Amphora:  0 rows
```
