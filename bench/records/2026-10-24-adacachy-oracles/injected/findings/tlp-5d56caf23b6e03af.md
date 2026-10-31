# tlp Wrong: the query's rows aren't the UNION of its partitions'

seed 20261024, state 805

## Statements

```sql
CREATE TABLE t2(c0 TEXT COLLATE BINARY, c1 REAL COLLATE RTRIM);
INSERT OR IGNORE INTO t2 VALUES (NULL, '1');
```

## Queries

```sql
SELECT NULL FROM t2 AS a0;
SELECT NULL FROM t2 AS a0 WHERE (NULL AND 'A%') UNION SELECT NULL FROM t2 AS a0 WHERE (NOT (NULL AND 'A%')) UNION SELECT NULL FROM t2 AS a0 WHERE ((NULL AND 'A%') ISNULL);
```

## What differed

```
the query's rows aren't the UNION of its partitions'
  expected: 1 rows
    NULL
  Amphora:  0 rows
```
