# tlp Wrong: the query's rows aren't the UNION of its partitions'

seed 20261024, state 1707

## Statements

```sql
CREATE TABLE t0(c0 TEXT);
INSERT INTO t0 VALUES ('_');
CREATE TABLE t1(c0 NUMERIC);
```

## Queries

```sql
SELECT NULL FROM t1 AS a0 RIGHT JOIN t0 AS a2 ON NULL;
SELECT NULL FROM t1 AS a0 RIGHT JOIN t0 AS a2 ON NULL WHERE a0.c0 UNION SELECT NULL FROM t1 AS a0 RIGHT JOIN t0 AS a2 ON NULL WHERE (NOT a0.c0) UNION SELECT NULL FROM t1 AS a0 RIGHT JOIN t0 AS a2 ON NULL WHERE (a0.c0 ISNULL);
```

## What differed

```
the query's rows aren't the UNION of its partitions'
  expected: 1 rows
    NULL
  Amphora:  0 rows
```
