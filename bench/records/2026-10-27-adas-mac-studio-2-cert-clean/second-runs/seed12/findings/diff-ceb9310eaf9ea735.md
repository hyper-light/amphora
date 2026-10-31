# diff Wrong: Amphora's answer isn't SQLite's

seed 12, state 71413

## Statements

```sql
CREATE TABLE t0(c0 BLOB COLLATE RTRIM);
INSERT INTO t0 VALUES (NULL);
```

## Queries

```sql
SELECT jsonb_set(atanh(0.5)) FROM t0 AS a0;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    x'c513302e3534393330363134343333343035343839'
  Amphora:  1 rows
    x'c513302e3534393330363134343333343035343738'
```
