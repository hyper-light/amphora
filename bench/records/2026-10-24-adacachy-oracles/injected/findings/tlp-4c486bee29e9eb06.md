# tlp Wrong: the query's rows aren't its three partitions' (p, NOT p, p IS NULL)

seed 20261024, state 89

## Statements

```sql
CREATE TABLE t0(c0 BLOB NOT NULL, c1 INT COLLATE BINARY NOT NULL, c2 VARCHAR(10) COLLATE RTRIM, c3 TEXT);
INSERT INTO t0 VALUES (0, 0, 9007199254740993, '1e3');
CREATE TABLE t1(c0 INT NOT NULL);
```

## Queries

```sql
SELECT NULL FROM t0 AS a0 LEFT JOIN t1 AS a1 ON NULL;
SELECT NULL FROM t0 AS a0 LEFT JOIN t1 AS a1 ON NULL WHERE a1.c0;
SELECT NULL FROM t0 AS a0 LEFT JOIN t1 AS a1 ON NULL WHERE (NOT a1.c0);
SELECT NULL FROM t0 AS a0 LEFT JOIN t1 AS a1 ON NULL WHERE (a1.c0 ISNULL);
```

## What differed

```
the query's rows aren't its three partitions' (p, NOT p, p IS NULL)
  expected: 1 rows
    NULL
  Amphora:  0 rows
```
