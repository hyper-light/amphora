# diff Wrong: Amphora's answer isn't SQLite's

seed 27005, state 2050

## Statements

```sql
CREATE TABLE t0(c0  COLLATE BINARY PRIMARY KEY, c1 DECIMAL(10,2), c2 INT, c3 REAL UNIQUE);
INSERT INTO t0 VALUES ('1', NULL, x'00', NULL);
```

## Queries

```sql
WITH w1(c0, c1, c2) AS (SELECT NULL, quote(asinh(1)), 9007199254740993 FROM t0 AS b13x0) SELECT octet_length(a0.c1) FROM w1 AS a0;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    19
  Amphora:  1 rows
    17
```
