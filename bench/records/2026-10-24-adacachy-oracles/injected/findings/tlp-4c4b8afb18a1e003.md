# tlp Wrong: the query's rows aren't the UNION of its partitions'

seed 20261024, state 210

## Statements

```sql
CREATE TABLE t1(c0 INT, c1 INT COLLATE BINARY PRIMARY KEY, c2 INT NOT NULL);
INSERT INTO t1 VALUES ('ABC', 'true', 9007199254740993);
```

## Queries

```sql
SELECT NULL FROM t1 AS a0 FULL JOIN t1 AS a1 ON NULL;
SELECT NULL FROM t1 AS a0 FULL JOIN t1 AS a1 ON NULL WHERE 1.5 UNION SELECT NULL FROM t1 AS a0 FULL JOIN t1 AS a1 ON NULL WHERE (NOT 1.5) UNION SELECT NULL FROM t1 AS a0 FULL JOIN t1 AS a1 ON NULL WHERE (1.5 ISNULL);
```

## What differed

```
the query's rows aren't the UNION of its partitions'
  expected: 2 rows
    NULL
    NULL
  Amphora:  1 rows
    NULL
```
