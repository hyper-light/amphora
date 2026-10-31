# diff Wrong: Amphora's answer isn't SQLite's

seed 12, state 4681

## Statements

```sql
CREATE TABLE t0(c0 TEXT, c1  PRIMARY KEY);
INSERT INTO t0 VALUES (0, 0.5);
```

## Queries

```sql
SELECT json_set(atanh(a0.c1)) FROM t0 AS a0;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    '0.54930614433405489'
  Amphora:  1 rows
    '0.54930614433405478'
```
