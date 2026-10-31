# diff Wrong: Amphora's answer isn't SQLite's

seed 12, state 1796

## Statements

```sql
CREATE TABLE t0(c0 VARCHAR(10));
INSERT INTO t0 VALUES (1e308);
```

## Queries

```sql
SELECT (CASE WHEN (SELECT min(s735.c0) FROM t0 AS s735 WHERE (a0.c0 > a0.c0)) THEN a0.c0 WHEN (9223372036854775807 IN (a0.c0, a1.c0)) THEN CAST('' AS BLOB) ELSE zeroblob(a0.c0) END) FROM t0 AS a0, t0 AS a1;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    x'00'
  Amphora:  1 rows
    x''
```
