# diff Wrong: Amphora's answer isn't SQLite's

seed 12, state 35235

## Statements

```sql
CREATE TABLE t0(c0  COLLATE NOCASE PRIMARY KEY);
INSERT INTO t0 VALUES (0.5);
```

## Queries

```sql
SELECT rtrim(atanh(a2.c0)) FROM t0 AS a0 FULL JOIN t0 AS a1 ON ((a0.c0 <> -100) = 0.1), t0 AS a2;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 2 rows
    '0.54930614433405489'
    '0.54930614433405489'
  Amphora:  2 rows
    '0.54930614433405478'
    '0.54930614433405478'
```
