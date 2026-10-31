# tlp Wrong: the query's rows aren't the UNION of its partitions'

seed 20261024, state 221

## Statements

```sql
CREATE TABLE t0(c0 INT COLLATE BINARY PRIMARY KEY);
INSERT INTO t0 VALUES (0);
```

## Queries

```sql
SELECT (a1.c0 IS a2.c0) FROM t0 AS a0, t0 AS a1 LEFT JOIN t0 AS a2 ON (atan2(a0.c0, a2.c0) LIKE '[a-z]*');
SELECT (a1.c0 IS a2.c0) FROM t0 AS a0, t0 AS a1 LEFT JOIN t0 AS a2 ON (atan2(a0.c0, a2.c0) LIKE '[a-z]*') WHERE (((a2.c0 << -9223372036854775808) * (NULL * a0.c0)) < ((CASE a2.c0 WHEN (a2.c0 ISNULL) THEN a1.c0 ELSE 0.5 END) COLLATE RTRIM)) UNION SELECT (a1.c0 IS a2.c0) FROM t0 AS a0, t0 AS a1 LEFT JOIN t0 AS a2 ON (atan2(a0.c0, a2.c0) LIKE '[a-z]*') WHERE (NOT (((a2.c0 << -9223372036854775808) * (NULL * a0.c0)) < ((CASE a2.c0 WHEN (a2.c0 ISNULL) THEN a1.c0 ELSE 0.5 END) COLLATE RTRIM))) UNION SELECT (a1.c0 IS a2.c0) FROM t0 AS a0, t0 AS a1 LEFT JOIN t0 AS a2 ON (atan2(a0.c0, a2.c0) LIKE '[a-z]*') WHERE ((((a2.c0 << -9223372036854775808) * (NULL * a0.c0)) < ((CASE a2.c0 WHEN (a2.c0 ISNULL) THEN a1.c0 ELSE 0.5 END) COLLATE RTRIM)) ISNULL);
```

## What differed

```
the query's rows aren't the UNION of its partitions'
  expected: 1 rows
    0
  Amphora:  0 rows
```
