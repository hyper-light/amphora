# eet Wrong: an equivalent form gives other rows

seed 20261028, state 54539

## Statements

```sql
CREATE TABLE t0(c0 INTEGER PRIMARY KEY);
INSERT OR IGNORE INTO t0 VALUES (-9223372036854775808);
CREATE TABLE t1(c0 TEXT COLLATE RTRIM, c1 INT COLLATE NOCASE, c2 BLOB COLLATE NOCASE, c3 VARCHAR(10));
INSERT OR IGNORE INTO t1 VALUES (x'', 'ABC', '_', -9223372036854775808);
```

## Queries

```sql
SELECT sin(255), count(*) FROM t1 AS a0 CROSS JOIN t0 AS a1, t0 AS a2 WHERE (CAST(CAST(a0.c3 AS REAL) AS TEXT) = a2.c0) GROUP BY sin(255);
SELECT sin(255), count(*) FROM t1 AS a0 CROSS JOIN t0 AS a1, t0 AS a2 WHERE (CASE WHEN (('0x10' OR (NOT (CASE WHEN (('ABC' AND (NOT 'ABC')) AND ('ABC' NOTNULL)) THEN 0 ELSE '0x10' END))) OR ('0x10' ISNULL)) THEN (CAST(CAST(a0.c3 AS REAL) AS TEXT) = a2.c0) ELSE NULL END) GROUP BY sin(255);
```

## What differed

```
an equivalent form gives other rows
  expected: 0 rows
  Amphora:  1 rows
    -0.5063916349244909, 1
```
