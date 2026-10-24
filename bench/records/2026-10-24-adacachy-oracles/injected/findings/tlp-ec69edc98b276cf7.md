# tlp Wrong: the query's aggregate isn't its partitions' combined

seed 20261024, state 923

## Statements

```sql
CREATE TABLE t0(c0 DOUBLE, c1 INT PRIMARY KEY);
INSERT INTO t0 VALUES ('0', ' ');
CREATE TABLE t1(c0 BOOLEAN COLLATE RTRIM PRIMARY KEY);
INSERT INTO t1 VALUES (NULL);
CREATE TABLE t2(c0 VARCHAR(10) NOT NULL, c1 INTEGER COLLATE BINARY, c2 DECIMAL(10,2) COLLATE NOCASE, c3 VARCHAR(10) UNIQUE);
INSERT INTO t2 VALUES (x'', -9223372036854775808, '-1', 1000000000000000.0);
```

## Queries

```sql
SELECT sum(((a1.c1 != a2.c0) COLLATE NOCASE)) FROM t1 AS a0, t0 AS a1, t2 AS a2;
SELECT sum(m) FROM (SELECT NULL AS m WHERE 0 UNION ALL SELECT sum(((a1.c1 != a2.c0) COLLATE NOCASE)) FROM t1 AS a0, t0 AS a1, t2 AS a2 WHERE ((a0.c0 <= a0.c0) NOT LIKE 'a%') UNION ALL SELECT sum(((a1.c1 != a2.c0) COLLATE NOCASE)) FROM t1 AS a0, t0 AS a1, t2 AS a2 WHERE (NOT ((a0.c0 <= a0.c0) NOT LIKE 'a%')) UNION ALL SELECT sum(((a1.c1 != a2.c0) COLLATE NOCASE)) FROM t1 AS a0, t0 AS a1, t2 AS a2 WHERE (((a0.c0 <= a0.c0) NOT LIKE 'a%') ISNULL));
```

## What differed

```
the query's aggregate isn't its partitions' combined
  expected: 1 rows
    1
  Amphora:  1 rows
    2
```
