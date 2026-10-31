# tlp Wrong: the query's rows aren't its three partitions' (p, NOT p, p IS NULL)

seed 20261024, state 713

## Statements

```sql
CREATE TABLE t0(c0 , c1 BLOB);
INSERT OR IGNORE INTO t0 VALUES (NULL, '0');
```

## Queries

```sql
SELECT json_array_insert((x'0102' < 'abc')) FROM t0 AS a0, t0 AS a1;
SELECT json_array_insert((x'0102' < 'abc')) FROM t0 AS a0, t0 AS a1 WHERE (((a1.c0 NOT LIKE 'A%') OR (-9223372036854775808 IN (NULL, a1.c0))) AND ((NOT (a0.c1 IS NOT DISTINCT FROM -9223372036854775808)) AND ((a1.c0 IS NOT DISTINCT FROM a0.c1) AND (a0.c1 NOT GLOB 'A%'))));
SELECT json_array_insert((x'0102' < 'abc')) FROM t0 AS a0, t0 AS a1 WHERE (NOT (((a1.c0 NOT LIKE 'A%') OR (-9223372036854775808 IN (NULL, a1.c0))) AND ((NOT (a0.c1 IS NOT DISTINCT FROM -9223372036854775808)) AND ((a1.c0 IS NOT DISTINCT FROM a0.c1) AND (a0.c1 NOT GLOB 'A%')))));
SELECT json_array_insert((x'0102' < 'abc')) FROM t0 AS a0, t0 AS a1 WHERE ((((a1.c0 NOT LIKE 'A%') OR (-9223372036854775808 IN (NULL, a1.c0))) AND ((NOT (a0.c1 IS NOT DISTINCT FROM -9223372036854775808)) AND ((a1.c0 IS NOT DISTINCT FROM a0.c1) AND (a0.c1 NOT GLOB 'A%')))) ISNULL);
```

## What differed

```
the query's rows aren't its three partitions' (p, NOT p, p IS NULL)
  expected: 1 rows
    '0'
  Amphora:  0 rows
```
