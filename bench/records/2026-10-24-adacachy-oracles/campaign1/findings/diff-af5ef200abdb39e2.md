# diff Wrong: Amphora's answer isn't SQLite's

seed 11, state 732

## Statements

```sql
CREATE TABLE t2(c0 INTEGER);
INSERT OR IGNORE INTO t2 VALUES (9223372036854775807);
```

## Queries

```sql
SELECT json_array(NULL, 255, CAST(a0.c0 AS REAL)) FROM t2 AS a0;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    '[null,255,9.2233720368547758e+18]'
  Amphora:  1 rows
    '[null,255,9.223372036854776e+18]'
```
