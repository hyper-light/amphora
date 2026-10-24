# tlp Wrong: the query's rows aren't its three partitions' (p, NOT p, p IS NULL)

seed 20261024, state 564

## Statements

```sql
CREATE TABLE t0(c0 REAL PRIMARY KEY NOT NULL, c1 DECIMAL(10,2));
INSERT INTO t0 VALUES ('x', 0);
CREATE TABLE t1(c0  UNIQUE, c1 NUMERIC PRIMARY KEY, c2 DOUBLE UNIQUE, c3 REAL COLLATE BINARY UNIQUE);
```

## Queries

```sql
SELECT 3.0 FROM t1 AS a0 RIGHT JOIN t0 AS a1 ON NULL LEFT JOIN t1 AS a2 ON '{"a":null}';
SELECT 3.0 FROM t1 AS a0 RIGHT JOIN t0 AS a1 ON NULL LEFT JOIN t1 AS a2 ON '{"a":null}' WHERE a2.c0;
SELECT 3.0 FROM t1 AS a0 RIGHT JOIN t0 AS a1 ON NULL LEFT JOIN t1 AS a2 ON '{"a":null}' WHERE (NOT a2.c0);
SELECT 3.0 FROM t1 AS a0 RIGHT JOIN t0 AS a1 ON NULL LEFT JOIN t1 AS a2 ON '{"a":null}' WHERE (a2.c0 ISNULL);
```

## What differed

```
the query's rows aren't its three partitions' (p, NOT p, p IS NULL)
  expected: 1 rows
    3.0
  Amphora:  0 rows
```
