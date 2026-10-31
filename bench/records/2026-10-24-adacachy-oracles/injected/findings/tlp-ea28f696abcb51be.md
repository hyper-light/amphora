# tlp Wrong: the query's rows aren't its three partitions' (p, NOT p, p IS NULL)

seed 20261024, state 1567

## Statements

```sql
CREATE TABLE t1(c0 INT COLLATE NOCASE, c1 TEXT PRIMARY KEY);
INSERT INTO t1 VALUES (-9223372036854775808, x'00');
CREATE TABLE t2(c0 REAL COLLATE BINARY PRIMARY KEY, c1 INTEGER, c2 BOOLEAN NOT NULL);
```

## Queries

```sql
SELECT NULL FROM t1 AS a0 FULL JOIN t2 AS a1 ON 'A';
SELECT NULL FROM t1 AS a0 FULL JOIN t2 AS a1 ON 'A' WHERE a1.c0;
SELECT NULL FROM t1 AS a0 FULL JOIN t2 AS a1 ON 'A' WHERE (NOT a1.c0);
SELECT NULL FROM t1 AS a0 FULL JOIN t2 AS a1 ON 'A' WHERE (a1.c0 ISNULL);
```

## What differed

```
the query's rows aren't its three partitions' (p, NOT p, p IS NULL)
  expected: 1 rows
    NULL
  Amphora:  0 rows
```
