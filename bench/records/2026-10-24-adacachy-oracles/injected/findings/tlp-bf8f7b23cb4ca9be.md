# tlp Wrong: the query's aggregate isn't its partitions' combined

seed 20261024, state 723

## Statements

```sql
CREATE TABLE t0(c0 NUMERIC, c1 VARCHAR(10) PRIMARY KEY);
CREATE TABLE t1(c0 INT);
```

## Queries

```sql
SELECT ((a1.c1 IS NOT DISTINCT FROM a1.c1) COLLATE BINARY) FROM t1 AS a0, t0 AS a1;
SELECT min(m COLLATE BINARY) FROM (SELECT NULL AS m WHERE 0 UNION ALL SELECT ((a1.c1 IS NOT DISTINCT FROM a1.c1) COLLATE BINARY) FROM t1 AS a0, t0 AS a1 WHERE ((julianday(a0.c0, a1.c1) || CAST(a1.c0 AS TEXT)) IN (a0.c0, 1000000000000000.0)) UNION ALL SELECT ((a1.c1 IS NOT DISTINCT FROM a1.c1) COLLATE BINARY) FROM t1 AS a0, t0 AS a1 WHERE (NOT ((julianday(a0.c0, a1.c1) || CAST(a1.c0 AS TEXT)) IN (a0.c0, 1000000000000000.0))) UNION ALL SELECT ((a1.c1 IS NOT DISTINCT FROM a1.c1) COLLATE BINARY) FROM t1 AS a0, t0 AS a1 WHERE (((julianday(a0.c0, a1.c1) || CAST(a1.c0 AS TEXT)) IN (a0.c0, 1000000000000000.0)) ISNULL));
```

## What differed

```
the query's aggregate isn't its partitions' combined
  expected: 0 rows
  Amphora:  1 rows
    NULL
```
