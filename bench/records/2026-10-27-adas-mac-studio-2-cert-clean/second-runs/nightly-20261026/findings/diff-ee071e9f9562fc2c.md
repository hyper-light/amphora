# diff Wrong: Amphora's answer isn't SQLite's

seed 20261026, state 56478

## Statements

```sql
CREATE TABLE t0(c0 VARCHAR(10) PRIMARY KEY, c1 INTEGER, c2 TEXT UNIQUE, c3 DECIMAL(10,2));
INSERT OR IGNORE INTO t0 VALUES (0.5, 100, 'A', 10);
```

## Queries

```sql
SELECT jsonb_set(atanh(a1.c0)) FROM t0 AS a0, t0 AS a1;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    x'c513302e3534393330363134343333343035343839'
  Amphora:  1 rows
    x'c513302e3534393330363134343333343035343738'
```
