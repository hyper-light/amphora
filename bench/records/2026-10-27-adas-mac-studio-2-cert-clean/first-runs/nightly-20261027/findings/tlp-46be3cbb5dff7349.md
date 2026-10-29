# tlp Wrong: the query's aggregate isn't its partitions' combined

seed 20261027, state 55188

## Statements

```sql
CREATE TABLE t0(c0 );
INSERT OR IGNORE INTO t0 VALUES (-9223372036854775808);
INSERT INTO t0 VALUES ('x');
INSERT INTO t0 VALUES (1e308);
CREATE TABLE t2(c0  COLLATE BINARY PRIMARY KEY, c1 INT);
INSERT INTO t2 VALUES ('x', '1');
```

## Queries

```sql
SELECT sum((a1.c0 * (~ a0.c0))) FROM t0 AS a0 JOIN t0 AS a1 ON (NOT (3 >= 100)), t2 AS a2;
SELECT sum(m) FROM (SELECT NULL AS m WHERE 0 UNION ALL SELECT sum((a1.c0 * (~ a0.c0))) FROM t0 AS a0 JOIN t0 AS a1 ON (NOT (3 >= 100)), t2 AS a2 WHERE (((' a' < a0.c0) AND (a2.c0 NOT BETWEEN 255 AND a2.c1)) OR (json_quote(a2.c1) BETWEEN (a0.c0 >> a2.c0) AND (-9223372036854775808 IN (a0.c0, a2.c1)))) UNION ALL SELECT sum((a1.c0 * (~ a0.c0))) FROM t0 AS a0 JOIN t0 AS a1 ON (NOT (3 >= 100)), t2 AS a2 WHERE (NOT (((' a' < a0.c0) AND (a2.c0 NOT BETWEEN 255 AND a2.c1)) OR (json_quote(a2.c1) BETWEEN (a0.c0 >> a2.c0) AND (-9223372036854775808 IN (a0.c0, a2.c1))))) UNION ALL SELECT sum((a1.c0 * (~ a0.c0))) FROM t0 AS a0 JOIN t0 AS a1 ON (NOT (3 >= 100)), t2 AS a2 WHERE ((((' a' < a0.c0) AND (a2.c0 NOT BETWEEN 255 AND a2.c1)) OR (json_quote(a2.c1) BETWEEN (a0.c0 >> a2.c0) AND (-9223372036854775808 IN (a0.c0, a2.c1)))) ISNULL));
```

## What differed

```
the query's aggregate isn't its partitions' combined
  expected: 1 rows
    NULL
  Amphora:  1 rows
    -1e308
```
