# diff Wrong: Amphora's answer isn't SQLite's

seed 1013, state 1533

## Statements

```sql
CREATE TABLE t0(c0 REAL COLLATE RTRIM, c1 VARCHAR(10));
INSERT INTO t0 VALUES (100, 1000000000000000.0);
```

## Queries

```sql
SELECT json_pretty('{"a":1,"b":[2,3]}', a0.c0) FROM t0 AS a0;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    '{
100.0"a": 1,
100.0"b": [
100.0100.02,
100.0100.03
100.0]
}'
  Amphora:  1 rows
    '{
100"a": 1,
100"b": [
1001002,
1001003
100]
}'
```
