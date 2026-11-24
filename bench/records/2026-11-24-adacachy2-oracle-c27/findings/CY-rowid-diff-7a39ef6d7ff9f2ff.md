# diff Wrong: Amphora's answer isn't SQLite's

seed 27007, state 10367

## Statements

```sql
CREATE TABLE t0(c0 INT, c1 DECIMAL(10,2) UNIQUE, c2 INTEGER COLLATE RTRIM PRIMARY KEY);
CREATE TABLE t1(c0 TEXT COLLATE BINARY, c1 REAL PRIMARY KEY);
CREATE TABLE lg(c0, c1, c2);
INSERT INTO t1 VALUES ('a ', '');
INSERT INTO t1 VALUES ('a', '1.5');
```

## Queries

```sql
SELECT NULL, NULL FROM lg AS a0 UNION SELECT (CASE a0.c1 WHEN a0.c1 THEN a0.c0 WHEN NULL THEN 10 END), NULL FROM t1 AS a0 GROUP BY a0.c1, a0.c0 UNION SELECT a0.c2, NULL FROM t0 AS a0;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 2 rows
    'a', NULL
    'a ', NULL
  Amphora:  1 rows
    'a ', NULL
```
