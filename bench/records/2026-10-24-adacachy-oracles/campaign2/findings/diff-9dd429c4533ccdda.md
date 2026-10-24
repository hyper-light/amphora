# diff Wrong: Amphora's answer isn't SQLite's

seed 12, state 1298

## Statements

```sql
CREATE TABLE t0(c0 INT, c1 TEXT COLLATE BINARY, c2 TEXT, c3 BOOLEAN COLLATE RTRIM UNIQUE NOT NULL);
INSERT INTO t0 VALUES (0.1, 0.0, 2.0, 2);
```

## Queries

```sql
SELECT replace(255, x'', NULL) FROM t0 AS a0;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    '255'
  Amphora:  1 rows
    NULL
```
