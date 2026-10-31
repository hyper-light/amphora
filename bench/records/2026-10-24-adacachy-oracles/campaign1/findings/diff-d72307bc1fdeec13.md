# diff Wrong: Amphora's answer isn't SQLite's

seed 11, state 809

## Statements

```sql
CREATE TABLE t0(c0 DOUBLE COLLATE NOCASE, c1 VARCHAR(10));
INSERT INTO t0 VALUES (NULL, '1.5'), (' ', 255), ('1', 'a ');
```

## Queries

```sql
SELECT -1e-308 FROM t0 AS a0 FULL JOIN t0 AS a1 ON (x'0102' NOT LIKE '%');
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 6 rows
    -1e-308
    -1e-308
    -1e-308
    -1e-308
    -1e-308
    -1e-308
  Amphora:  9 rows
    -1e-308
    -1e-308
    -1e-308
    -1e-308
    -1e-308
    -1e-308
    ...
```
