# diff Wrong: Amphora's answer isn't SQLite's

seed 20261024, state 609

## Statements

```sql
CREATE TABLE t0(c0 DOUBLE, c1  NOT NULL, c2 NUMERIC PRIMARY KEY, c3 BOOLEAN);
INSERT INTO t0 VALUES (3, 9223372036854775807, NULL, ' ');
CREATE TABLE t1(c0  COLLATE BINARY, c1 DOUBLE NOT NULL, c2 INT PRIMARY KEY);
INSERT INTO t1 VALUES (NULL, -1e-308, 1e308);
```

## Queries

```sql
SELECT CAST(3 AS TEXT), count(*) FROM t1 AS a0, t0 AS a1 WHERE ((NOT (a1.c2 NOT LIKE '_')) AND (EXISTS (SELECT 1 FROM t1 AS s50 WHERE ('ABC' NOT IN (s50.c2, '0x10'))))) GROUP BY CAST(3 AS TEXT);
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 0 rows
  Amphora:  1 rows
    '3', 1
```
