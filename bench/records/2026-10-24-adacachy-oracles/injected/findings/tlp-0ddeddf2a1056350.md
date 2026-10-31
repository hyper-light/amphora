# tlp Wrong: the query's rows aren't its three partitions' (p, NOT p, p IS NULL)

seed 20261024, state 1231

## Statements

```sql
CREATE TABLE t0(c0 DECIMAL(10,2) COLLATE RTRIM UNIQUE NOT NULL);
CREATE TABLE t1(c0 INT COLLATE NOCASE PRIMARY KEY, c1 NUMERIC, c2 REAL, c3  COLLATE BINARY);
INSERT OR IGNORE INTO t1 VALUES ('0', 1e308, NULL, NULL);
```

## Queries

```sql
SELECT ((a2.c1 << x'ff') & (a2.c1 < a0.c0)) FROM t0 AS a0, t1 AS a1 RIGHT JOIN t1 AS a2 ON ((a2.c0 COLLATE RTRIM) > (a0.c0 < a1.c1));
SELECT ((a2.c1 << x'ff') & (a2.c1 < a0.c0)) FROM t0 AS a0, t1 AS a1 RIGHT JOIN t1 AS a2 ON ((a2.c0 COLLATE RTRIM) > (a0.c0 < a1.c1)) WHERE ((a0.c0 - ('' IS NOT DISTINCT FROM a1.c0)) BETWEEN a1.c2 AND zeroblob(log2(2.0)));
SELECT ((a2.c1 << x'ff') & (a2.c1 < a0.c0)) FROM t0 AS a0, t1 AS a1 RIGHT JOIN t1 AS a2 ON ((a2.c0 COLLATE RTRIM) > (a0.c0 < a1.c1)) WHERE (NOT ((a0.c0 - ('' IS NOT DISTINCT FROM a1.c0)) BETWEEN a1.c2 AND zeroblob(log2(2.0))));
SELECT ((a2.c1 << x'ff') & (a2.c1 < a0.c0)) FROM t0 AS a0, t1 AS a1 RIGHT JOIN t1 AS a2 ON ((a2.c0 COLLATE RTRIM) > (a0.c0 < a1.c1)) WHERE (((a0.c0 - ('' IS NOT DISTINCT FROM a1.c0)) BETWEEN a1.c2 AND zeroblob(log2(2.0))) ISNULL);
```

## What differed

```
the query's rows aren't its three partitions' (p, NOT p, p IS NULL)
  expected: 1 rows
    NULL
  Amphora:  0 rows
```
