# diff Wrong: Amphora's answer isn't SQLite's

seed 20261026, state 60664

## Statements

```sql
CREATE TABLE t0(c0 DECIMAL(10,2) PRIMARY KEY);
INSERT INTO t0 VALUES (0.5);
CREATE TABLE t1(c0 VARCHAR(10), c1 INT, c2 BOOLEAN PRIMARY KEY NOT NULL);
INSERT INTO t1 VALUES (1000000000000000.0, x'00', 'x');
```

## Queries

```sql
SELECT NULL FROM t1 AS a0 JOIN t0 AS a2 ON ((a2.c0 || a0.c1) IN (SELECT s96.c0 FROM t0 AS s96));
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    NULL
  Amphora:  0 rows
```
