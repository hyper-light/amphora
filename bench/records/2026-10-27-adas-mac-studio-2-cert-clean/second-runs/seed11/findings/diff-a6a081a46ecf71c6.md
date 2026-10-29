# diff Wrong: Amphora's answer isn't SQLite's

seed 11, state 71347

## Statements

```sql
CREATE TABLE t0(c0  UNIQUE);
INSERT OR IGNORE INTO t0 VALUES (0.5);
```

## Queries

```sql
SELECT CAST(atanh(a0.c0) AS BLOB) FROM t0 AS a0;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    x'302e3534393330363134343333343035343839'
  Amphora:  1 rows
    x'302e3534393330363134343333343035343738'
```
