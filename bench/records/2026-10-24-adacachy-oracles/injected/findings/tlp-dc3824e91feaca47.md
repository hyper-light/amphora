# tlp Wrong: the query's aggregate isn't its partitions' combined

seed 20261024, state 634

## Statements

```sql
CREATE TABLE t1(c0 INT COLLATE RTRIM PRIMARY KEY, c1 BLOB UNIQUE);
```

## Queries

```sql
SELECT NULL FROM t1 AS a0;
SELECT sum(m) FROM (SELECT NULL AS m WHERE 0 UNION ALL SELECT NULL FROM t1 AS a0 WHERE NULL UNION ALL SELECT NULL FROM t1 AS a0 WHERE (NOT NULL) UNION ALL SELECT NULL FROM t1 AS a0 WHERE (NULL ISNULL));
```

## What differed

```
the query's aggregate isn't its partitions' combined
  expected: 0 rows
  Amphora:  1 rows
    NULL
```
