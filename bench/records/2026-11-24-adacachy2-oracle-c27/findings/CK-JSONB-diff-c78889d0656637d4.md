# diff Wrong: Amphora's answer isn't SQLite's

seed 27005, state 18481

## Statements

```sql
CREATE TABLE t2(c0 BLOB UNIQUE, c1 INTEGER);
INSERT OR IGNORE INTO t2 VALUES (0.5, 2);
```

## Queries

```sql
SELECT jsonb_array(sin(a0.c0)) FROM t2 AS a0;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    x'cb15c513302e3437393432353533383630343230333031'
  Amphora:  1 rows
    x'cb13c511302e343739343235353338363034323033'
```
