# diff Wrong: Amphora's answer isn't SQLite's

seed 8001, state 17227

## Statements

```sql
CREATE TABLE t0(c0 TEXT NOT NULL) STRICT;
CREATE TABLE t1(c0 NUMERIC, c1 NUMERIC COLLATE NOCASE);
CREATE TABLE t2(c0 INTEGER, c1 DECIMAL(10,2) PRIMARY KEY, c2 REAL UNIQUE);
INSERT INTO t0 VALUES ('%');
INSERT INTO t1 VALUES (9007199254740993, 'A');
INSERT OR IGNORE INTO t2 VALUES (3.0, 9007199254740993, 0.5);
```

## Queries

```sql
SELECT concat_ws((a1.c0 - 0), ln(a2.c0), (a0.c0 NOT BETWEEN x'0102' AND a0.c2)) FROM t2 AS a0 CROSS JOIN t0 AS a1, t1 AS a2;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    '36.73680056967710101'
  Amphora:  1 rows
    '36.736800569677101'
```
