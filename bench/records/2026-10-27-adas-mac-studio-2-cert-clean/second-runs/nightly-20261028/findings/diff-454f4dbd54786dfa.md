# diff Wrong: Amphora's answer isn't SQLite's

seed 20261028, state 73

## Statements

```sql
CREATE TABLE t0(c0 TEXT UNIQUE);
INSERT OR IGNORE INTO t0 VALUES (0.5);
```

## Queries

```sql
SELECT CAST(atanh(a2.c0) AS TEXT) FROM t0 AS a0 LEFT JOIN t0 AS a1 ON ('é' >= (- a1.c0)), t0 AS a2;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    '0.54930614433405489'
  Amphora:  1 rows
    '0.54930614433405478'
```
