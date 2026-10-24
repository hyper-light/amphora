# tlp Wrong: the query's rows aren't its three partitions' (p, NOT p, p IS NULL)

seed 20261024, state 693

## Statements

```sql
CREATE TABLE t0(c0 INT COLLATE NOCASE PRIMARY KEY, c1 BOOLEAN COLLATE RTRIM);
CREATE TABLE t1(c0 REAL, c1 INTEGER PRIMARY KEY, c2 DOUBLE);
INSERT OR IGNORE INTO t1 VALUES (1, 10, 1000000000000000.0);
```

## Queries

```sql
SELECT a0.c0 FROM t0 AS a0 FULL JOIN t1 AS a1 ON (SELECT sum(s470.c1) FROM t1 AS s470 WHERE ((3 IN (-100)) AND ('1' != a0.c1))), t1 AS a2;
SELECT a0.c0 FROM t0 AS a0 FULL JOIN t1 AS a1 ON (SELECT sum(s470.c1) FROM t1 AS s470 WHERE ((3 IN (-100)) AND ('1' != a0.c1))), t1 AS a2 WHERE (a0.c1 NOT BETWEEN json_extract('{"a":1,"b":[2,3]}', '$') AND (CASE 'A' WHEN (a2.c0 IS DISTINCT FROM a0.c0) THEN ('-1' COLLATE NOCASE) ELSE a2.c1 END));
SELECT a0.c0 FROM t0 AS a0 FULL JOIN t1 AS a1 ON (SELECT sum(s470.c1) FROM t1 AS s470 WHERE ((3 IN (-100)) AND ('1' != a0.c1))), t1 AS a2 WHERE (NOT (a0.c1 NOT BETWEEN json_extract('{"a":1,"b":[2,3]}', '$') AND (CASE 'A' WHEN (a2.c0 IS DISTINCT FROM a0.c0) THEN ('-1' COLLATE NOCASE) ELSE a2.c1 END)));
SELECT a0.c0 FROM t0 AS a0 FULL JOIN t1 AS a1 ON (SELECT sum(s470.c1) FROM t1 AS s470 WHERE ((3 IN (-100)) AND ('1' != a0.c1))), t1 AS a2 WHERE ((a0.c1 NOT BETWEEN json_extract('{"a":1,"b":[2,3]}', '$') AND (CASE 'A' WHEN (a2.c0 IS DISTINCT FROM a0.c0) THEN ('-1' COLLATE NOCASE) ELSE a2.c1 END)) ISNULL);
```

## What differed

```
the query's rows aren't its three partitions' (p, NOT p, p IS NULL)
  expected: 1 rows
    NULL
  Amphora:  0 rows
```
