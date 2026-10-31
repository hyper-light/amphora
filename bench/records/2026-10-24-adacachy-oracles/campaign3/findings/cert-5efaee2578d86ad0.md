# cert Estimate: the more restrictive query is estimated at 1000000 rows, the query at 900000

seed 13, state 1755

## Statements

```sql
CREATE TABLE t0(c0 , c1 BLOB NOT NULL);
```

## Queries

```sql
SELECT 0.5 FROM t0 AS a0 WHERE (-9223372036854775808 IS NOT a0.c1);
SELECT 0.5 FROM t0 AS a0 WHERE ((-9223372036854775808 IS NOT a0.c1) AND ('{"a":1,"b":[2,3]}' NOT IN (SELECT s708.c1 FROM t0 AS s708 WHERE (NOT EXISTS (SELECT 1 FROM t0 AS s337 WHERE (s708.c1 != s337.c0))))));
```

## What differed

```
the more restrictive query is estimated at 1000000 rows, the query at 900000
```
