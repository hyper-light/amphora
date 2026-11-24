# diff Wrong: Amphora's answer isn't SQLite's

seed 8001, state 4555

## Statements

```sql
CREATE TABLE t0(c0 NUMERIC COLLATE NOCASE);
INSERT INTO t0 VALUES (-1);
```

## Queries

```sql
SELECT jsonb_array(asinh(1), NULL, NULL) FROM t0 AS a0;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    x'cb17c513302e38383133373335383730313935343330350000'
  Amphora:  1 rows
    x'cb15c511302e3838313337333538373031393534330000'
```
