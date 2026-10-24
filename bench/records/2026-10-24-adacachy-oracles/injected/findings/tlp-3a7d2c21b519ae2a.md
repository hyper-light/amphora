# tlp Wrong: the query's aggregate isn't its partitions' combined

seed 20261024, state 1268

## Statements

```sql
CREATE TABLE t0(c0 TEXT, c1 INT, c2 NUMERIC COLLATE BINARY);
INSERT OR IGNORE INTO t0 VALUES (7, 1000000000000000.0, 1e308);
CREATE TABLE t1(c0 INT, c1 VARCHAR(10), c2 NUMERIC COLLATE NOCASE, c3 NUMERIC);
INSERT OR IGNORE INTO t1 VALUES ('1e3', 3, '%', ' a');
```

## Queries

```sql
SELECT sum(ifnull(a2.c3, (NULL NOTNULL))) FROM t0 AS a0 LEFT JOIN t1 AS a1 ON ((a1.c1 % a1.c0) IS (a1.c2 / a0.c2)), t1 AS a2;
SELECT sum(m) FROM (SELECT NULL AS m WHERE 0 UNION ALL SELECT sum(ifnull(a2.c3, (NULL NOTNULL))) FROM t0 AS a0 LEFT JOIN t1 AS a1 ON ((a1.c1 % a1.c0) IS (a1.c2 / a0.c2)), t1 AS a2 WHERE (a1.c3 BETWEEN (a1.c1 COLLATE NOCASE) AND (json_extract('[1,2,3]', '$') - time(a2.c0, 0.0, a1.c2))) UNION ALL SELECT sum(ifnull(a2.c3, (NULL NOTNULL))) FROM t0 AS a0 LEFT JOIN t1 AS a1 ON ((a1.c1 % a1.c0) IS (a1.c2 / a0.c2)), t1 AS a2 WHERE (NOT (a1.c3 BETWEEN (a1.c1 COLLATE NOCASE) AND (json_extract('[1,2,3]', '$') - time(a2.c0, 0.0, a1.c2)))) UNION ALL SELECT sum(ifnull(a2.c3, (NULL NOTNULL))) FROM t0 AS a0 LEFT JOIN t1 AS a1 ON ((a1.c1 % a1.c0) IS (a1.c2 / a0.c2)), t1 AS a2 WHERE ((a1.c3 BETWEEN (a1.c1 COLLATE NOCASE) AND (json_extract('[1,2,3]', '$') - time(a2.c0, 0.0, a1.c2))) ISNULL));
```

## What differed

```
the query's aggregate isn't its partitions' combined
  expected: 1 rows
    0.0
  Amphora:  1 rows
    NULL
```
