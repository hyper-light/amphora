# cert Estimate: the more restrictive query is estimated at 106.48000000000002 rows, the query at 9.680014697496697

seed 14, state 12798

## Statements

```sql
CREATE TABLE t0(c0 TEXT, c1 BLOB, c2 VARCHAR(10) PRIMARY KEY, c3 );
INSERT INTO t0 VALUES (9007199254740993, 0.1, NULL, '1e3');
INSERT INTO t0 VALUES ('a ', 9223372036854775807, 'x', 3.0);
INSERT OR IGNORE INTO t0 VALUES (3, 'abc', NULL, '%');
INSERT INTO t0 VALUES (x'', 1.5, ' ', 7);
INSERT OR IGNORE INTO t0 VALUES (-2.5, 9007199254740993, 2.0, 10), (1e308, 1, 0.0, NULL), (10, 'ABC', NULL, '10');
INSERT OR IGNORE INTO t0 VALUES (7, x'00', 255, NULL);
INSERT INTO t0 VALUES (7, 0.5, '%', '%'), (' ', -1, x'', 10), (9223372036854775807, 'a ', 1e308, 9223372036854775807);
ANALYZE;
```

## Queries

```sql
SELECT NULL FROM t0 AS a0 JOIN t0 AS a1 ON ((2 * a0.c1) >= (NULL >= a1.c3)) LEFT JOIN t0 AS a2 ON 3.0 WHERE (a1.c2 GLOB '_');
SELECT NULL FROM t0 AS a0 JOIN t0 AS a1 ON ((2 * a0.c1) >= (NULL >= a1.c3)) JOIN t0 AS a2 ON 3.0 WHERE (a1.c2 GLOB '_');
```

## What differed

```
the more restrictive query is estimated at 106.48000000000002 rows, the query at 9.680014697496697
```
