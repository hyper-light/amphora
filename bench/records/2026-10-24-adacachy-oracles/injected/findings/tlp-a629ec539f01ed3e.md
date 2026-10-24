# tlp Wrong: the query's rows aren't the UNION of its partitions'

seed 20261024, state 1272

## Statements

```sql
CREATE TABLE t0(c0 BLOB UNIQUE);
INSERT OR IGNORE INTO t0 VALUES (x'0102');
```

## Queries

```sql
SELECT NULL FROM t0 AS a0 RIGHT JOIN t0 AS a1 ON '%';
SELECT NULL FROM t0 AS a0 RIGHT JOIN t0 AS a1 ON '%' WHERE a0.c0 UNION SELECT NULL FROM t0 AS a0 RIGHT JOIN t0 AS a1 ON '%' WHERE (NOT a0.c0) UNION SELECT NULL FROM t0 AS a0 RIGHT JOIN t0 AS a1 ON '%' WHERE (a0.c0 ISNULL);
```

## What differed

```
the query's rows aren't the UNION of its partitions'
  expected: 1 rows
    NULL
  Amphora:  0 rows
```
