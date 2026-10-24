# cert Estimate: the more restrictive query is estimated at 8 rows, the query at 6.4

seed 14, state 15939

## Statements

```sql
CREATE TABLE t0(c0 INTEGER COLLATE BINARY, c1 NUMERIC);
INSERT OR IGNORE INTO t0 VALUES ('a ', 0.5);
INSERT OR IGNORE INTO t0 VALUES (7, 'a');
INSERT OR IGNORE INTO t0 VALUES (-1, 100), (9007199254740993, '1'), ('ABC', -100);
INSERT OR IGNORE INTO t0 VALUES (NULL, 'a'), (9223372036854775807, -1e-308), ('1e3', 3.0);
ANALYZE;
```

## Queries

```sql
SELECT NULL FROM t0 AS a0 CROSS JOIN t0 AS a1 WHERE (a1.c1 IS NULL);
SELECT NULL FROM t0 AS a0 CROSS JOIN t0 AS a1 WHERE ((a1.c1 IS NULL) AND ((SELECT sum(s917.c0) FROM t0 AS s917 WHERE (a1.c0 GLOB '_')) OR ((NULL NOTNULL) OR (NULL >= NULL))));
```

## What differed

```
the more restrictive query is estimated at 8 rows, the query at 6.4
```
