# tlp Wrong: the query's rows aren't the UNION of its partitions'

seed 20261024, state 354

## Statements

```sql
CREATE TABLE t2(c0 DOUBLE);
INSERT INTO t2 VALUES (NULL);
```

## Queries

```sql
SELECT NULL FROM t2 AS a0 LEFT JOIN t2 AS a1 ON 'é';
SELECT NULL FROM t2 AS a0 LEFT JOIN t2 AS a1 ON 'é' WHERE a1.c0 UNION SELECT NULL FROM t2 AS a0 LEFT JOIN t2 AS a1 ON 'é' WHERE (NOT a1.c0) UNION SELECT NULL FROM t2 AS a0 LEFT JOIN t2 AS a1 ON 'é' WHERE (a1.c0 ISNULL);
```

## What differed

```
the query's rows aren't the UNION of its partitions'
  expected: 1 rows
    NULL
  Amphora:  0 rows
```
