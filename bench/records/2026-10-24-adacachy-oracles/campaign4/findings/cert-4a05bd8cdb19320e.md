# cert Estimate: the more restrictive query is estimated at 22.5 rows, the query at 20

seed 14, state 8820

## Statements

```sql
CREATE TABLE t0(c0 DOUBLE COLLATE NOCASE, c1 DOUBLE COLLATE RTRIM, c2  COLLATE RTRIM, c3 VARCHAR(10) PRIMARY KEY NOT NULL);
INSERT OR IGNORE INTO t0 VALUES (3, '%', x'00', 9007199254740993);
INSERT INTO t0 VALUES (1, -100, 1000000000000000.0, x''), (1, 0, '0', -2.5), ('1e3', 3.0, 100, 10);
INSERT OR IGNORE INTO t0 VALUES (NULL, NULL, ' a', 'ABC');
CREATE INDEX i0 ON t0(c1);
ANALYZE;
```

## Queries

```sql
SELECT like(a2.c1, a0.c0) FROM t0 AS a0 JOIN t0 AS a1 ON ((0.5 COLLATE NOCASE) >= 9223372036854775807), t0 AS a2 WHERE (a1.c1 BETWEEN (a0.c1 >= a2.c2) AND (NOT a0.c2));
SELECT like(a2.c1, a0.c0) FROM t0 AS a0 JOIN t0 AS a1 ON ((0.5 COLLATE NOCASE) >= 9223372036854775807), t0 AS a2 WHERE ((a1.c1 BETWEEN (a0.c1 >= a2.c2) AND (NOT a0.c2)) AND (a0.c3 IS NOT (a0.c2 <> a2.c3)));
```

## What differed

```
the more restrictive query is estimated at 22.5 rows, the query at 20
```
