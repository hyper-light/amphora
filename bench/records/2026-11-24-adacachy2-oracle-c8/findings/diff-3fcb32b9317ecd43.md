# diff Wrong: Amphora's answer isn't SQLite's

seed 904, state 1695

## Statements

```sql
CREATE TABLE t0(c0 DECIMAL(10,2) PRIMARY KEY, c1  UNIQUE);
INSERT INTO t0 VALUES ('', 255);
CREATE TABLE t1(c0 , c1 DOUBLE, c2 INT UNIQUE, c3 );
```

## Queries

```sql
SELECT -9223372036854775808 FROM t0 AS a0 LEFT JOIN t1 AS a1 ON (a0.c0 < 1000000000000000.0) RIGHT JOIN t0 AS a2 ON (a0.c0 NOTNULL) WHERE (concat(7) > a0.c1);
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    -9223372036854775808
  Amphora:  0 rows
```
