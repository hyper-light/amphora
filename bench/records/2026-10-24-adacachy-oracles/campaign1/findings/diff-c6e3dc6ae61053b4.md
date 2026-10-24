# diff Wrong: Amphora's answer isn't SQLite's

seed 11, state 404

## Statements

```sql
CREATE TABLE t0(c0 DECIMAL(10,2), c1 NUMERIC PRIMARY KEY NOT NULL, c2 VARCHAR(10) COLLATE BINARY, c3 DOUBLE);
INSERT INTO t0 VALUES (9007199254740993, 2, 0, 255);
```

## Queries

```sql
SELECT replace((1e308 / a0.c0), 0, 0) FROM t0 AS a0;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    '1.1102230246251566e+292'
  Amphora:  1 rows
    '1.11022302462516e+292'
```
