# tlp Wrong: the query's aggregate isn't its partitions' combined

seed 20261024, state 1733

## Statements

```sql
CREATE TABLE t0(c0 NUMERIC COLLATE BINARY, c1 INTEGER COLLATE RTRIM PRIMARY KEY);
INSERT OR IGNORE INTO t0 VALUES (0.5, '-1');
CREATE TABLE t1(c0 NUMERIC PRIMARY KEY);
INSERT INTO t1 VALUES (x'00');
```

## Queries

```sql
SELECT min((a1.c1 COLLATE BINARY)) FROM t0 AS a0 FULL JOIN t0 AS a1 ON (9223372036854775807 IN ((a1.c0 + a1.c0), CAST(x'ff' AS TEXT))), t1 AS a2;
SELECT min(m COLLATE BINARY) FROM (SELECT NULL AS m WHERE 0 UNION ALL SELECT min((a1.c1 COLLATE BINARY)) FROM t0 AS a0 FULL JOIN t0 AS a1 ON (9223372036854775807 IN ((a1.c0 + a1.c0), CAST(x'ff' AS TEXT))), t1 AS a2 WHERE (('x' % (a0.c0 / a0.c1)) BETWEEN (a0.c1 NOT BETWEEN a2.c0 AND -1e-308) AND (a0.c1 NOT LIKE 'a*')) UNION ALL SELECT min((a1.c1 COLLATE BINARY)) FROM t0 AS a0 FULL JOIN t0 AS a1 ON (9223372036854775807 IN ((a1.c0 + a1.c0), CAST(x'ff' AS TEXT))), t1 AS a2 WHERE (NOT (('x' % (a0.c0 / a0.c1)) BETWEEN (a0.c1 NOT BETWEEN a2.c0 AND -1e-308) AND (a0.c1 NOT LIKE 'a*'))) UNION ALL SELECT min((a1.c1 COLLATE BINARY)) FROM t0 AS a0 FULL JOIN t0 AS a1 ON (9223372036854775807 IN ((a1.c0 + a1.c0), CAST(x'ff' AS TEXT))), t1 AS a2 WHERE ((('x' % (a0.c0 / a0.c1)) BETWEEN (a0.c1 NOT BETWEEN a2.c0 AND -1e-308) AND (a0.c1 NOT LIKE 'a*')) ISNULL));
```

## What differed

```
the query's aggregate isn't its partitions' combined
  expected: 1 rows
    -1
  Amphora:  1 rows
    NULL
```
