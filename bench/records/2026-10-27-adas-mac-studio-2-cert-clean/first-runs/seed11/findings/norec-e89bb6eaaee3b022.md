# norec Panic: cursor id 1 is None

seed 11, state 25833

## Statements

```sql
CREATE TABLE t0(c0 INT, c1 INTEGER UNIQUE, c2 BLOB PRIMARY KEY);
INSERT INTO t0 VALUES (1.5, 9223372036854775807, 3);
```

## Queries

```sql
SELECT (CASE WHEN (NOT x'41') THEN a1.c0 WHEN (1.5 != 2) THEN (a1.c2 || 1000000000000000.0) ELSE ('1' IN (a1.c0, a1.c0, a1.c0)) END) FROM t0 AS a0, t0 AS a1 RIGHT JOIN t0 AS a2 ON (a1.c1 >= time(2.0, -2.5));
SELECT (CASE WHEN (NOT x'41') THEN a1.c0 WHEN (1.5 != 2) THEN (a1.c2 || 1000000000000000.0) ELSE ('1' IN (a1.c0, a1.c0, a1.c0)) END) FROM t0 AS a0, t0 AS a1 RIGHT JOIN t0 AS a2 ON (a1.c1 >= time(2.0, -2.5)) WHERE (((+ a0.c1) NOT GLOB 'A%') OR (SELECT max(s338.c0) FROM t0 AS s338 WHERE ('0x10' BETWEEN s338.c2 AND s338.c1)));
SELECT (CASE WHEN (NOT x'41') THEN a1.c0 WHEN (1.5 != 2) THEN (a1.c2 || 1000000000000000.0) ELSE ('1' IN (a1.c0, a1.c0, a1.c0)) END) FROM t0 AS a0, t0 AS a1 RIGHT JOIN t0 AS a2 ON (a1.c1 >= time(2.0, -2.5)) WHERE (NOT (((+ a0.c1) NOT GLOB 'A%') OR (SELECT max(s338.c0) FROM t0 AS s338 WHERE ('0x10' BETWEEN s338.c2 AND s338.c1))));
SELECT (CASE WHEN (NOT x'41') THEN a1.c0 WHEN (1.5 != 2) THEN (a1.c2 || 1000000000000000.0) ELSE ('1' IN (a1.c0, a1.c0, a1.c0)) END) FROM t0 AS a0, t0 AS a1 RIGHT JOIN t0 AS a2 ON (a1.c1 >= time(2.0, -2.5)) WHERE ((((+ a0.c1) NOT GLOB 'A%') OR (SELECT max(s338.c0) FROM t0 AS s338 WHERE ('0x10' BETWEEN s338.c2 AND s338.c1))) ISNULL);
```

## What differed

```
cursor id 1 is None
```
