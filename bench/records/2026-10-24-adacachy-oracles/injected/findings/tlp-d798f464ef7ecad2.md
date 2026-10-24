# tlp Wrong: the query's rows aren't the UNION of its partitions'

seed 20261024, state 1623

## Statements

```sql
CREATE TABLE t0(c0 INTEGER COLLATE RTRIM, c1 INT, c2  COLLATE BINARY NOT NULL, c3 BOOLEAN);
CREATE TABLE t2(c0 VARCHAR(10) PRIMARY KEY, c1 DOUBLE, c2 NUMERIC, c3 DOUBLE COLLATE RTRIM UNIQUE);
INSERT INTO t2 VALUES (3.0, '0x10', NULL, 2.0);
```

## Queries

```sql
SELECT 0.1, '10' FROM t2 AS a0 LEFT JOIN t0 AS a1 ON x'ff' GROUP BY 0.1, NULL;
SELECT 0.1, '10' FROM t2 AS a0 LEFT JOIN t0 AS a1 ON x'ff' WHERE a1.c3 GROUP BY 0.1, NULL UNION SELECT 0.1, '10' FROM t2 AS a0 LEFT JOIN t0 AS a1 ON x'ff' WHERE (NOT a1.c3) GROUP BY 0.1, NULL UNION SELECT 0.1, '10' FROM t2 AS a0 LEFT JOIN t0 AS a1 ON x'ff' WHERE (a1.c3 ISNULL) GROUP BY 0.1, NULL;
```

## What differed

```
the query's rows aren't the UNION of its partitions'
  expected: 1 rows
    0.1, '10'
  Amphora:  0 rows
```
