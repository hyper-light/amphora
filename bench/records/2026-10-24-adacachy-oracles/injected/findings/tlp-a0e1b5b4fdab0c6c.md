# tlp Wrong: the query's rows aren't the UNION of its partitions'

seed 20261024, state 1135

## Statements

```sql
CREATE TABLE t0(c0 TEXT, c1  PRIMARY KEY);
CREATE TABLE t1(c0 VARCHAR(10) UNIQUE, c1 DOUBLE, c2 DECIMAL(10,2), c3 DOUBLE COLLATE BINARY);
INSERT INTO t1 VALUES (0.5, 3.0, x'ff', 3.0);
```

## Queries

```sql
SELECT (like(-100, a2.c2) || json_extract('{"a":1,"b":[2,3]}', '$.a')) FROM t1 AS a0, t0 AS a1 RIGHT JOIN t1 AS a2 ON (SELECT sum(s35.c0) FROM t1 AS s35 WHERE (s35.c0 ISNULL));
SELECT (like(-100, a2.c2) || json_extract('{"a":1,"b":[2,3]}', '$.a')) FROM t1 AS a0, t0 AS a1 RIGHT JOIN t1 AS a2 ON (SELECT sum(s35.c0) FROM t1 AS s35 WHERE (s35.c0 ISNULL)) WHERE (a0.c1 IN ((a1.c0 != NULL), (a1.c0 IN ('1e3')))) UNION SELECT (like(-100, a2.c2) || json_extract('{"a":1,"b":[2,3]}', '$.a')) FROM t1 AS a0, t0 AS a1 RIGHT JOIN t1 AS a2 ON (SELECT sum(s35.c0) FROM t1 AS s35 WHERE (s35.c0 ISNULL)) WHERE (NOT (a0.c1 IN ((a1.c0 != NULL), (a1.c0 IN ('1e3'))))) UNION SELECT (like(-100, a2.c2) || json_extract('{"a":1,"b":[2,3]}', '$.a')) FROM t1 AS a0, t0 AS a1 RIGHT JOIN t1 AS a2 ON (SELECT sum(s35.c0) FROM t1 AS s35 WHERE (s35.c0 ISNULL)) WHERE ((a0.c1 IN ((a1.c0 != NULL), (a1.c0 IN ('1e3')))) ISNULL);
```

## What differed

```
the query's rows aren't the UNION of its partitions'
  expected: 1 rows
    '01'
  Amphora:  0 rows
```
