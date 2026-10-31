# diff Wrong: Amphora's answer isn't SQLite's

seed 20261024, state 382

## Statements

```sql
CREATE TABLE t0(c0 INTEGER, c1 DOUBLE);
INSERT INTO t0 VALUES (-1e-308, 0.0);
```

## Queries

```sql
SELECT log(json_extract('{"a":1,"b":[2,3]}', '$.b[1]'), ('1e3' || a0.c1)) FROM t0 AS a0;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    62.87709822868153
  Amphora:  1 rows
    NULL
```
