# tlp Wrong: the query's rows aren't the UNION of its partitions'

seed 20261024, state 1527

## Statements

```sql
CREATE TABLE t0(c0 BOOLEAN COLLATE BINARY, c1 , c2 NUMERIC);
INSERT INTO t0 VALUES ('x', '1.5', 0);
CREATE TABLE t1(c0 VARCHAR(10) COLLATE BINARY);
```

## Queries

```sql
SELECT NULL FROM t0 AS a0 LEFT JOIN t1 AS a1 ON NULL;
SELECT NULL FROM t0 AS a0 LEFT JOIN t1 AS a1 ON NULL WHERE a1.c0 UNION SELECT NULL FROM t0 AS a0 LEFT JOIN t1 AS a1 ON NULL WHERE (NOT a1.c0) UNION SELECT NULL FROM t0 AS a0 LEFT JOIN t1 AS a1 ON NULL WHERE (a1.c0 ISNULL);
```

## What differed

```
the query's rows aren't the UNION of its partitions'
  expected: 1 rows
    NULL
  Amphora:  0 rows
```
