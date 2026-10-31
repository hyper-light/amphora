# cert Estimate: the more restrictive query is estimated at 16000000000.000004 rows, the query at 5000000000

seed 14, state 4063

## Statements

```sql
CREATE TABLE t0(c0 BOOLEAN, c1 TEXT COLLATE RTRIM PRIMARY KEY, c2 NUMERIC, c3 VARCHAR(10));
```

## Queries

```sql
SELECT sinh(a1.c0) FROM t0 AS a0, t0 AS a1 WHERE (a0.c1 BETWEEN json_extract('{"a":1,"b":[2,3]}', '$[0]') AND a1.c1);
SELECT sinh(a1.c0) FROM t0 AS a0, t0 AS a1 WHERE ((a0.c1 BETWEEN json_extract('{"a":1,"b":[2,3]}', '$[0]') AND a1.c1) AND ((7 <= '1e3') == a0.c0));
```

## What differed

```
the more restrictive query is estimated at 16000000000.000004 rows, the query at 5000000000
```
