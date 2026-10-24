# tlp Wrong: the query's aggregate isn't its partitions' combined

seed 20261024, state 715

## Statements

```sql
CREATE TABLE t1(c0 INTEGER, c1 INT UNIQUE);
INSERT OR IGNORE INTO t1 VALUES (3, NULL);
```

## Queries

```sql
SELECT count(2) FROM t1 AS a0, t1 AS a1;
SELECT sum(m) FROM (SELECT NULL AS m WHERE 0 UNION ALL SELECT count(2) FROM t1 AS a0, t1 AS a1 WHERE ((('1.5' IN (a1.c1, a0.c1, a0.c0)) AND (a0.c0 NOT BETWEEN a0.c1 AND a0.c0)) AND (a1.c0 IS (1000000000000000.0 ISNULL))) UNION ALL SELECT count(2) FROM t1 AS a0, t1 AS a1 WHERE (NOT ((('1.5' IN (a1.c1, a0.c1, a0.c0)) AND (a0.c0 NOT BETWEEN a0.c1 AND a0.c0)) AND (a1.c0 IS (1000000000000000.0 ISNULL)))) UNION ALL SELECT count(2) FROM t1 AS a0, t1 AS a1 WHERE (((('1.5' IN (a1.c1, a0.c1, a0.c0)) AND (a0.c0 NOT BETWEEN a0.c1 AND a0.c0)) AND (a1.c0 IS (1000000000000000.0 ISNULL))) ISNULL));
```

## What differed

```
the query's aggregate isn't its partitions' combined
  expected: 1 rows
    1
  Amphora:  1 rows
    0
```
