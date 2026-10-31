# tlp Wrong: the query's rows aren't the UNION of its partitions'

seed 20261024, state 113

## Statements

```sql
CREATE TABLE t0(c0 INT COLLATE BINARY UNIQUE NOT NULL, c1 VARCHAR(10), c2 BLOB COLLATE RTRIM UNIQUE NOT NULL);
CREATE TABLE t2(c0 VARCHAR(10) COLLATE RTRIM);
INSERT INTO t2 VALUES (7);
```

## Queries

```sql
SELECT NULL FROM t2 AS a0 LEFT JOIN t0 AS a1 ON NULL;
SELECT NULL FROM t2 AS a0 LEFT JOIN t0 AS a1 ON NULL WHERE a1.c1 UNION SELECT NULL FROM t2 AS a0 LEFT JOIN t0 AS a1 ON NULL WHERE (NOT a1.c1) UNION SELECT NULL FROM t2 AS a0 LEFT JOIN t0 AS a1 ON NULL WHERE (a1.c1 ISNULL);
```

## What differed

```
the query's rows aren't the UNION of its partitions'
  expected: 1 rows
    NULL
  Amphora:  0 rows
```
