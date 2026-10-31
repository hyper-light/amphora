# tlp Wrong: the query's aggregate isn't its partitions' combined

seed 20261024, state 604

## Statements

```sql
CREATE TABLE t0(c0 DECIMAL(10,2) UNIQUE, c1 TEXT, c2 INTEGER, c3 DECIMAL(10,2) COLLATE BINARY PRIMARY KEY NOT NULL);
```

## Queries

```sql
SELECT '1.5' FROM t0 AS a0, t0 AS a1;
SELECT sum(m) FROM (SELECT NULL AS m WHERE 0 UNION ALL SELECT '1.5' FROM t0 AS a0, t0 AS a1 WHERE ((~ (a1.c3 COLLATE NOCASE)) ISNULL) UNION ALL SELECT '1.5' FROM t0 AS a0, t0 AS a1 WHERE (NOT ((~ (a1.c3 COLLATE NOCASE)) ISNULL)) UNION ALL SELECT '1.5' FROM t0 AS a0, t0 AS a1 WHERE (((~ (a1.c3 COLLATE NOCASE)) ISNULL) ISNULL));
```

## What differed

```
the query's aggregate isn't its partitions' combined
  expected: 0 rows
  Amphora:  1 rows
    NULL
```
