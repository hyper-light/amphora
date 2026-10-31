# diff Wrong: Amphora's answer isn't SQLite's

seed 11, state 326

## Statements

```sql
CREATE TABLE t0(c0 TEXT, c1 DECIMAL(10,2), c2 VARCHAR(10));
INSERT OR IGNORE INTO t0 VALUES (x'00', NULL, '_');
INSERT OR IGNORE INTO t0 VALUES (x'0102', 'abc', 1e308);
```

## Queries

```sql
SELECT (a1.c1 IS NOT a0.c2), json_extract('[1,2,3]', '$.b[1]'), count(*) FROM t0 AS a0, t0 AS a1 WHERE ((a1.c2 IN (a0.c0, a1.c2)) NOT BETWEEN CAST((CASE 1e308 WHEN a1.c2 THEN a0.c2 WHEN ('1e3' IN (a1.c1, -9223372036854775808)) THEN a1.c0 END) AS TEXT) AND ((NOT -100) OR (a0.c1 ISNULL))) GROUP BY (a1.c1 IS NOT a0.c2), json_extract('[1,2,3]', '$.b[1]');
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    1, NULL, 3
  Amphora:  1 rows
    1, NULL, 2
```
