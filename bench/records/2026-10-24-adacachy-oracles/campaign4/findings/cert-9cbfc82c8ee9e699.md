# cert Estimate: the more restrictive query is estimated at 25600000000.000004 rows, the query at 5000000000

seed 14, state 24618

## Statements

```sql
CREATE TABLE t0(c0 BLOB PRIMARY KEY);
```

## Queries

```sql
SELECT '1e3' FROM t0 AS a0, t0 AS a1 WHERE ((a0.c0 BETWEEN quote(a1.c0) AND CAST(1e308 AS INTEGER)) AND ('a' = CAST(x'41' AS BLOB)));
SELECT '1e3' FROM t0 AS a0, t0 AS a1 WHERE (((a0.c0 BETWEEN quote(a1.c0) AND CAST(1e308 AS INTEGER)) AND ('a' = CAST(x'41' AS BLOB))) AND (julianday(a1.c0, x'0102', a1.c0) BETWEEN (0.0 = a0.c0) AND ('0' NOTNULL)));
```

## What differed

```
the more restrictive query is estimated at 25600000000.000004 rows, the query at 5000000000
```
