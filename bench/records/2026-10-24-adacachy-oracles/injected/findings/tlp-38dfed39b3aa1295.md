# tlp Wrong: the query's rows aren't its three partitions' (p, NOT p, p IS NULL)

seed 20261024, state 720

## Statements

```sql
CREATE TABLE t0(c0 TEXT UNIQUE);
INSERT OR IGNORE INTO t0 VALUES (' ');
CREATE TABLE t1(c0 DECIMAL(10,2) PRIMARY KEY, c1 DECIMAL(10,2));
CREATE TABLE t2(c0 INT, c1 NUMERIC, c2 INT, c3 DECIMAL(10,2) UNIQUE);
INSERT OR IGNORE INTO t2 VALUES (NULL, '', '0', 255);
```

## Queries

```sql
SELECT 'ABC' FROM t0 AS a0 LEFT JOIN t1 AS a1 ON ((a0.c0 BETWEEN a1.c1 AND a1.c0) OR (3.0 BETWEEN a1.c1 AND a0.c0)), t2 AS a2;
SELECT 'ABC' FROM t0 AS a0 LEFT JOIN t1 AS a1 ON ((a0.c0 BETWEEN a1.c1 AND a1.c0) OR (3.0 BETWEEN a1.c1 AND a0.c0)), t2 AS a2 WHERE ((a1.c0 <= a0.c0) = ((~ a0.c0) >> ceil(a2.c1)));
SELECT 'ABC' FROM t0 AS a0 LEFT JOIN t1 AS a1 ON ((a0.c0 BETWEEN a1.c1 AND a1.c0) OR (3.0 BETWEEN a1.c1 AND a0.c0)), t2 AS a2 WHERE (NOT ((a1.c0 <= a0.c0) = ((~ a0.c0) >> ceil(a2.c1))));
SELECT 'ABC' FROM t0 AS a0 LEFT JOIN t1 AS a1 ON ((a0.c0 BETWEEN a1.c1 AND a1.c0) OR (3.0 BETWEEN a1.c1 AND a0.c0)), t2 AS a2 WHERE (((a1.c0 <= a0.c0) = ((~ a0.c0) >> ceil(a2.c1))) ISNULL);
```

## What differed

```
the query's rows aren't its three partitions' (p, NOT p, p IS NULL)
  expected: 1 rows
    'ABC'
  Amphora:  0 rows
```
