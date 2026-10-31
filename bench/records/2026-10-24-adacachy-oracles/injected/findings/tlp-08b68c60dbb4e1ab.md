# tlp Wrong: the query's rows aren't the UNION of its partitions'

seed 20261024, state 383

## Statements

```sql
CREATE TABLE t0(c0 NUMERIC, c1 VARCHAR(10) UNIQUE);
INSERT OR IGNORE INTO t0 VALUES (0.0, 9007199254740993);
CREATE TABLE t2(c0 BOOLEAN COLLATE RTRIM);
```

## Queries

```sql
SELECT 0.0, NULL FROM t0 AS a0 LEFT JOIN t2 AS a1 ON '_' GROUP BY 0.0, NULL;
SELECT 0.0, NULL FROM t0 AS a0 LEFT JOIN t2 AS a1 ON '_' WHERE a1.c0 GROUP BY 0.0, NULL UNION SELECT 0.0, NULL FROM t0 AS a0 LEFT JOIN t2 AS a1 ON '_' WHERE (NOT a1.c0) GROUP BY 0.0, NULL UNION SELECT 0.0, NULL FROM t0 AS a0 LEFT JOIN t2 AS a1 ON '_' WHERE (a1.c0 ISNULL) GROUP BY 0.0, NULL;
```

## What differed

```
the query's rows aren't the UNION of its partitions'
  expected: 1 rows
    0.0, NULL
  Amphora:  0 rows
```
