# tlp Wrong: the query's rows aren't its three partitions' (p, NOT p, p IS NULL)

seed 20261026, state 20082

## Statements

```sql
CREATE TABLE t0(c0 VARCHAR(10) COLLATE NOCASE, c1 VARCHAR(10) COLLATE NOCASE, c2 DECIMAL(10,2));
INSERT INTO t0 VALUES (10, 0.0, NULL);
INSERT INTO t0 VALUES ('%', 0, 0);
CREATE TABLE t2(c0 VARCHAR(10) COLLATE BINARY PRIMARY KEY NOT NULL);
INSERT INTO t2 VALUES (0);
```

## Queries

```sql
SELECT a0.c0, floor(a2.c1), count(*) FROM t2 AS a0 RIGHT JOIN t0 AS a1, t0 AS a2 GROUP BY a0.c0, floor(a2.c1);
SELECT a0.c0, floor(a2.c1), count(*) FROM t2 AS a0 RIGHT JOIN t0 AS a1, t0 AS a2 GROUP BY a0.c0, floor(a2.c1) HAVING (floor(a2.c1) IS a0.c0);
SELECT a0.c0, floor(a2.c1), count(*) FROM t2 AS a0 RIGHT JOIN t0 AS a1, t0 AS a2 GROUP BY a0.c0, floor(a2.c1) HAVING (NOT (floor(a2.c1) IS a0.c0));
SELECT a0.c0, floor(a2.c1), count(*) FROM t2 AS a0 RIGHT JOIN t0 AS a1, t0 AS a2 GROUP BY a0.c0, floor(a2.c1) HAVING ((floor(a2.c1) IS a0.c0) ISNULL);
```

## What differed

```
the query's rows aren't its three partitions' (p, NOT p, p IS NULL)
  expected: 1 rows
    '0', 0.0, 4
  Amphora:  2 rows
    '0', 0, 2
    '0', 0.0, 2
```
