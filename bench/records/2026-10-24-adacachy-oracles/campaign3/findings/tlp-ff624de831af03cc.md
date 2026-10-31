# tlp Wrong: the query's rows aren't the UNION of its partitions'

seed 13, state 20830

## Statements

```sql
CREATE TABLE t0(c0 DOUBLE, c1 TEXT, c2 NUMERIC COLLATE RTRIM);
INSERT INTO t0 VALUES (NULL, 7, 2), (2, 'ABC', '1.5');
```

## Queries

```sql
SELECT DISTINCT (CASE '1' WHEN 1 THEN 0 WHEN a0.c2 THEN 'true' END) FROM t0 AS a0;
SELECT DISTINCT (CASE '1' WHEN 1 THEN 0 WHEN a0.c2 THEN 'true' END) FROM t0 AS a0 WHERE NULL UNION SELECT DISTINCT (CASE '1' WHEN 1 THEN 0 WHEN a0.c2 THEN 'true' END) FROM t0 AS a0 WHERE (NOT NULL) UNION SELECT DISTINCT (CASE '1' WHEN 1 THEN 0 WHEN a0.c2 THEN 'true' END) FROM t0 AS a0 WHERE (NULL ISNULL);
```

## What differed

```
the query's rows aren't the UNION of its partitions'
  expected: 2 rows
    NULL
    0
  Amphora:  1 rows
    NULL
```
