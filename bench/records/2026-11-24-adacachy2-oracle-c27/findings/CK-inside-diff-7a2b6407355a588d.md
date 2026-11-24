# diff Wrong: Amphora's answer isn't SQLite's

seed 27010, state 14563

## Statements

```sql
```

## Queries

```sql
WITH RECURSIVE w0(c0) AS (SELECT CAST(1 AS INTEGER) UNION SELECT CAST((w0.c0 * 3) AS INTEGER) FROM w0 AS w0) SELECT replace(sqrt(a0.c0), 0, a0.c0) FROM w0 AS a0;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 41 rows
    '1.1'
    '1.7323538375688772'
    '3.9'
    '5.1961524227276632'
    '9.81'
    '15.588457268119896'
    ...
  Amphora:  41 rows
    '1.1'
    '1.7323538375688772'
    '3.9'
    '5.1961524227276632'
    '9.81'
    '15.588457268119896'
    ...
```
