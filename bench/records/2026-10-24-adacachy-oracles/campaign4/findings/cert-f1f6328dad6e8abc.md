# cert Estimate: the more restrictive query is estimated at 25600000000.000008 rows, the query at 5000000000

seed 14, state 2538

## Statements

```sql
CREATE TABLE t1(c0 TEXT COLLATE NOCASE PRIMARY KEY, c1 VARCHAR(10) COLLATE NOCASE, c2 INTEGER);
```

## Queries

```sql
SELECT CAST((a0.c2 || '_') AS NUMERIC) FROM t1 AS a0, t1 AS a1 WHERE (a1.c0 BETWEEN json_quote(json_extract('{"a":1,"b":[2,3]}', '$')) AND ('-1' <> a0.c1));
SELECT CAST((a0.c2 || '_') AS NUMERIC) FROM t1 AS a0, t1 AS a1 WHERE ((a1.c0 BETWEEN json_quote(json_extract('{"a":1,"b":[2,3]}', '$')) AND ('-1' <> a0.c1)) AND ((a1.c1 IS NOT DISTINCT FROM 1) BETWEEN json_extract('{"a":null}', '$[0]') AND (CASE 0.1 WHEN (x'c3bf' IS DISTINCT FROM '1.5') THEN a0.c1 WHEN (NULL >= a1.c0) THEN '%' ELSE a1.c1 END)));
```

## What differed

```
the more restrictive query is estimated at 25600000000.000008 rows, the query at 5000000000
```
