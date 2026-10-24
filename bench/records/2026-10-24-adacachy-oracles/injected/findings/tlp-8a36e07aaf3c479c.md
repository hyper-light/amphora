# tlp Wrong: the query's aggregate isn't its partitions' combined

seed 20261024, state 674

## Statements

```sql
CREATE TABLE t0(c0 TEXT, c1 VARCHAR(10), c2  COLLATE RTRIM, c3 VARCHAR(10) COLLATE NOCASE);
```

## Queries

```sql
SELECT NULL FROM t0 AS a0;
SELECT sum(m) FROM (SELECT NULL AS m WHERE 0 UNION ALL SELECT NULL FROM t0 AS a0 WHERE NULL UNION ALL SELECT NULL FROM t0 AS a0 WHERE (NOT NULL) UNION ALL SELECT NULL FROM t0 AS a0 WHERE (NULL ISNULL));
```

## What differed

```
the query's aggregate isn't its partitions' combined
  expected: 0 rows
  Amphora:  1 rows
    NULL
```
