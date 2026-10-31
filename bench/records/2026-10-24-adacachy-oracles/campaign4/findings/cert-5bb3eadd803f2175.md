# cert Estimate: the more restrictive query is estimated at 24999.999999999996 rows, the query at 21353.35283236613

seed 14, state 22566

## Statements

```sql
CREATE TABLE t1(c0 DECIMAL(10,2) COLLATE BINARY, c1 DECIMAL(10,2) COLLATE BINARY, c2 BOOLEAN);
CREATE TABLE t2(c0 TEXT COLLATE NOCASE, c1 INT PRIMARY KEY, c2 INTEGER COLLATE NOCASE, c3 DECIMAL(10,2));
INSERT INTO t2 VALUES ('a', NULL, '10', 7), ('', 'ABC', 9007199254740993, -9223372036854775808);
ANALYZE;
```

## Queries

```sql
SELECT NULL FROM t2 AS a0, t1 AS a1 RIGHT JOIN t2 AS a2 ON (-9223372036854775808 GLOB 'A%') WHERE (a2.c1 BETWEEN json_extract((a1.c2 >> a0.c1)) AND (- json_extract('"x"', '$.a')));
SELECT NULL FROM t2 AS a0, t1 AS a1 JOIN t2 AS a2 ON (-9223372036854775808 GLOB 'A%') WHERE (a2.c1 BETWEEN json_extract((a1.c2 >> a0.c1)) AND (- json_extract('"x"', '$.a')));
```

## What differed

```
the more restrictive query is estimated at 24999.999999999996 rows, the query at 21353.35283236613
```
