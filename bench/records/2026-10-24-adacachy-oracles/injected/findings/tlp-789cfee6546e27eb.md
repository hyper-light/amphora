# tlp Wrong: the query's aggregate isn't its partitions' combined

seed 20261024, state 672

## Statements

```sql
CREATE TABLE t0(c0 BOOLEAN);
INSERT INTO t0 VALUES ('1e3');
CREATE TABLE t1(c0 TEXT COLLATE RTRIM PRIMARY KEY NOT NULL);
INSERT INTO t1 VALUES (x'41');
```

## Queries

```sql
SELECT count((a1.c0 IN (a1.c0))) FROM t0 AS a0, t1 AS a1;
SELECT sum(m) FROM (SELECT NULL AS m WHERE 0 UNION ALL SELECT count((a1.c0 IN (a1.c0))) FROM t0 AS a0, t1 AS a1 WHERE ((NOT (a1.c0 IN (9223372036854775807))) AND ((NULL NOT IN ('é', a1.c0, a1.c0)) AND (a0.c0 BETWEEN a1.c0 AND a0.c0))) UNION ALL SELECT count((a1.c0 IN (a1.c0))) FROM t0 AS a0, t1 AS a1 WHERE (NOT ((NOT (a1.c0 IN (9223372036854775807))) AND ((NULL NOT IN ('é', a1.c0, a1.c0)) AND (a0.c0 BETWEEN a1.c0 AND a0.c0)))) UNION ALL SELECT count((a1.c0 IN (a1.c0))) FROM t0 AS a0, t1 AS a1 WHERE (((NOT (a1.c0 IN (9223372036854775807))) AND ((NULL NOT IN ('é', a1.c0, a1.c0)) AND (a0.c0 BETWEEN a1.c0 AND a0.c0))) ISNULL));
```

## What differed

```
the query's aggregate isn't its partitions' combined
  expected: 1 rows
    1
  Amphora:  1 rows
    0
```
