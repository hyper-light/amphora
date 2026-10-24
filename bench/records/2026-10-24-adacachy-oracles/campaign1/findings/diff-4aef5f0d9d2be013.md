# diff Wrong: Amphora's answer isn't SQLite's

seed 11, state 37

## Statements

```sql
CREATE TABLE t0(c0 BOOLEAN UNIQUE, c1 BLOB UNIQUE NOT NULL);
INSERT OR IGNORE INTO t0 VALUES ('-1', 0.5);
```

## Queries

```sql
SELECT (0 || exp(1)) FROM t0 AS a0;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    '02.7182818284590451'
  Amphora:  1 rows
    '02.71828182845905'
```
