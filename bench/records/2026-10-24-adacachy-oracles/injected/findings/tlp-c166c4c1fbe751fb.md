# tlp Wrong: the query's rows aren't its three partitions' (p, NOT p, p IS NULL)

seed 20261024, state 530

## Statements

```sql
CREATE TABLE t0(c0 BLOB);
INSERT INTO t0 VALUES ('10');
CREATE TABLE t2(c0 TEXT, c1 INTEGER PRIMARY KEY, c2 INT COLLATE BINARY, c3 NUMERIC);
```

## Queries

```sql
SELECT a0.c0 FROM t0 AS a0, t2 AS a1 FULL JOIN t0 AS a2 ON (NOT (a1.c0 BETWEEN a0.c0 AND a2.c0));
SELECT a0.c0 FROM t0 AS a0, t2 AS a1 FULL JOIN t0 AS a2 ON (NOT (a1.c0 BETWEEN a0.c0 AND a2.c0)) WHERE ((json('1.5') >> a2.c0) >= a1.c2);
SELECT a0.c0 FROM t0 AS a0, t2 AS a1 FULL JOIN t0 AS a2 ON (NOT (a1.c0 BETWEEN a0.c0 AND a2.c0)) WHERE (NOT ((json('1.5') >> a2.c0) >= a1.c2));
SELECT a0.c0 FROM t0 AS a0, t2 AS a1 FULL JOIN t0 AS a2 ON (NOT (a1.c0 BETWEEN a0.c0 AND a2.c0)) WHERE (((json('1.5') >> a2.c0) >= a1.c2) ISNULL);
```

## What differed

```
the query's rows aren't its three partitions' (p, NOT p, p IS NULL)
  expected: 1 rows
    NULL
  Amphora:  0 rows
```
