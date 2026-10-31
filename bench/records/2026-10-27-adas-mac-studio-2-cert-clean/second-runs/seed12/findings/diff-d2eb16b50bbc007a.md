# diff Wrong: Amphora's answer isn't SQLite's

seed 12, state 74479

## Statements

```sql
CREATE TABLE t1(c0 BOOLEAN UNIQUE NOT NULL, c1  UNIQUE, c2 INT, c3 NUMERIC);
INSERT INTO t1 VALUES (10, 0.0, NULL, 7), (7, '10', 10, 'a '), ('a', '%', 0, 3);
CREATE TABLE t2(c0 DECIMAL(10,2) COLLATE RTRIM);
```

## Queries

```sql
SELECT 255 FROM t1 AS a0 RIGHT JOIN t1 AS a1 ON (a1.c2 IN (SELECT s315.c0 FROM t2 AS s315)) LIMIT 4 OFFSET 2;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    255
  Amphora:  0 rows
```
