# diff Wrong: Amphora's answer isn't SQLite's

seed 27004, state 19200

## Statements

```sql
CREATE TABLE t0(c0 NUMERIC);
```

## Queries

```sql
SELECT -2.5, 2 FROM t0 AS a0 UNION SELECT NULL, count(*) FROM t0 AS a0 GROUP BY NULL ORDER BY (1 COLLATE NOCASE);
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    NULL, 0
  Amphora:  0 rows
```
