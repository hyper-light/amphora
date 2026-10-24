# tlp Wrong: the query's rows aren't the UNION of its partitions'

seed 20261024, state 941

## Statements

```sql
CREATE TABLE t0(c0 DECIMAL(10,2), c1 INT PRIMARY KEY);
INSERT OR IGNORE INTO t0 VALUES (9007199254740993, 1.5);
```

## Queries

```sql
SELECT 9223372036854775807 FROM t0 AS a0;
SELECT 9223372036854775807 FROM t0 AS a0 WHERE (NULL AND 'a') UNION SELECT 9223372036854775807 FROM t0 AS a0 WHERE (NOT (NULL AND 'a')) UNION SELECT 9223372036854775807 FROM t0 AS a0 WHERE ((NULL AND 'a') ISNULL);
```

## What differed

```
the query's rows aren't the UNION of its partitions'
  expected: 1 rows
    9223372036854775807
  Amphora:  0 rows
```
