# tlp Wrong: the query's rows aren't the UNION of its partitions'

seed 20261024, state 825

## Statements

```sql
CREATE TABLE t0(c0 DOUBLE PRIMARY KEY, c1 BOOLEAN UNIQUE);
CREATE TABLE t1(c0 DECIMAL(10,2) COLLATE NOCASE UNIQUE, c1 BLOB COLLATE BINARY PRIMARY KEY, c2 INTEGER);
INSERT OR IGNORE INTO t1 VALUES (0.5, 1e308, -1e-308);
```

## Queries

```sql
SELECT NULL, '"x"' FROM t1 AS a0 LEFT JOIN t0 AS a1 ON NULL GROUP BY NULL, '"x"';
SELECT NULL, '"x"' FROM t1 AS a0 LEFT JOIN t0 AS a1 ON NULL WHERE a1.c1 GROUP BY NULL, '"x"' UNION SELECT NULL, '"x"' FROM t1 AS a0 LEFT JOIN t0 AS a1 ON NULL WHERE (NOT a1.c1) GROUP BY NULL, '"x"' UNION SELECT NULL, '"x"' FROM t1 AS a0 LEFT JOIN t0 AS a1 ON NULL WHERE (a1.c1 ISNULL) GROUP BY NULL, '"x"';
```

## What differed

```
the query's rows aren't the UNION of its partitions'
  expected: 1 rows
    NULL, '"x"'
  Amphora:  0 rows
```
