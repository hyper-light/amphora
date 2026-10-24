# cert Estimate: the more restrictive query is estimated at 160000000000.00003 rows, the query at 1000000

seed 12, state 52

## Statements

```sql
CREATE TABLE t0(c0 BOOLEAN COLLATE BINARY UNIQUE, c1  COLLATE BINARY PRIMARY KEY NOT NULL);
```

## Queries

```sql
SELECT json_extract('[1,2,3]', '$.a') FROM t0 AS a0, t0 AS a1 WHERE (a0.c0 IN (((a0.c1 IS DISTINCT FROM a0.c0) OR a1.c1)));
SELECT json_extract('[1,2,3]', '$.a') FROM t0 AS a0, t0 AS a1 WHERE ((a0.c0 IN (((a0.c1 IS DISTINCT FROM a0.c0) OR a1.c1))) AND (('ABC' || a1.c1) < (a1.c1 <= x'ff')));
```

## What differed

```
the more restrictive query is estimated at 160000000000.00003 rows, the query at 1000000
```
