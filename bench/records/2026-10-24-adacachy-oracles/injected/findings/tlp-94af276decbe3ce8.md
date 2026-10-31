# tlp Wrong: the query's rows aren't the UNION of its partitions'

seed 20261024, state 586

## Statements

```sql
CREATE TABLE t0(c0 TEXT, c1 TEXT PRIMARY KEY, c2 BOOLEAN NOT NULL, c3 REAL UNIQUE NOT NULL);
INSERT OR IGNORE INTO t0 VALUES (NULL, x'0102', 0.1, 10);
```

## Queries

```sql
SELECT NULL FROM t0 AS a0 FULL JOIN t0 AS a1 ON NULL;
SELECT NULL FROM t0 AS a0 FULL JOIN t0 AS a1 ON NULL WHERE '[1,2,3]' UNION SELECT NULL FROM t0 AS a0 FULL JOIN t0 AS a1 ON NULL WHERE (NOT '[1,2,3]') UNION SELECT NULL FROM t0 AS a0 FULL JOIN t0 AS a1 ON NULL WHERE ('[1,2,3]' ISNULL);
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
