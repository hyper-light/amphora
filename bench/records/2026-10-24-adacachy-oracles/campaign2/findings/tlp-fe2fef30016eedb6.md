# tlp Wrong: the query's rows aren't the UNION of its partitions'

seed 12, state 1653

## Statements

```sql
CREATE TABLE t0(c0 INT COLLATE BINARY, c1 DOUBLE PRIMARY KEY, c2 INTEGER COLLATE RTRIM, c3 INT UNIQUE);
INSERT OR IGNORE INTO t0 VALUES (NULL, 1.5, x'', 0.1);
INSERT OR IGNORE INTO t0 VALUES ('a ', '%', ' ', 1.5);
CREATE TABLE t1(c0 DECIMAL(10,2));
```

## Queries

```sql
SELECT printf(a2.c2, a2.c0, 0.0), '-1' FROM t1 AS a0, t1 AS a1 RIGHT JOIN t0 AS a2 GROUP BY printf(a2.c2, a2.c0, 0.0), '-1';
SELECT printf(a2.c2, a2.c0, 0.0), '-1' FROM t1 AS a0, t1 AS a1 RIGHT JOIN t0 AS a2 WHERE (NOT ((NOT (a2.c2 BETWEEN a1.c0 AND x'00')) OR (3 IS DISTINCT FROM a0.c0))) GROUP BY printf(a2.c2, a2.c0, 0.0), '-1' UNION SELECT printf(a2.c2, a2.c0, 0.0), '-1' FROM t1 AS a0, t1 AS a1 RIGHT JOIN t0 AS a2 WHERE (NOT (NOT ((NOT (a2.c2 BETWEEN a1.c0 AND x'00')) OR (3 IS DISTINCT FROM a0.c0)))) GROUP BY printf(a2.c2, a2.c0, 0.0), '-1' UNION SELECT printf(a2.c2, a2.c0, 0.0), '-1' FROM t1 AS a0, t1 AS a1 RIGHT JOIN t0 AS a2 WHERE ((NOT ((NOT (a2.c2 BETWEEN a1.c0 AND x'00')) OR (3 IS DISTINCT FROM a0.c0))) ISNULL) GROUP BY printf(a2.c2, a2.c0, 0.0), '-1';
```

## What differed

```
the query's rows aren't the UNION of its partitions'
  expected: 2 rows
    '', '-1'
    ' ', '-1'
  Amphora:  1 rows
    ' ', '-1'
```
