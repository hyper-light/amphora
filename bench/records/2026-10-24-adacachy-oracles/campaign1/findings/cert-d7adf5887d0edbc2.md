# cert Estimate: the more restrictive query is estimated at 1000000000000 rows, the query at 400000000000

seed 11, state 251

## Statements

```sql
CREATE TABLE t1(c0 DECIMAL(10,2), c1 VARCHAR(10) PRIMARY KEY NOT NULL);
```

## Queries

```sql
SELECT abs((+ a1.c0)) FROM t1 AS a0, t1 AS a1 JOIN t1 AS a2 ON (a2.c1 IN (json_extract('{"a":null}', '$[0]'))) WHERE ((a2.c1 == a1.c0) < ((a0.c0 IS DISTINCT FROM 'abc') COLLATE BINARY));
SELECT abs((+ a1.c0)) FROM t1 AS a0, t1 AS a1 JOIN t1 AS a2 ON (a2.c1 IN (json_extract('{"a":null}', '$[0]'))) WHERE (((a2.c1 == a1.c0) < ((a0.c0 IS DISTINCT FROM 'abc') COLLATE BINARY)) AND ((a0.c1 NOT IN ('')) IN ((2.0 IS NOT DISTINCT FROM a2.c0), json_extract('{"a":1,"b":[2,3]}', '$'), (a2.c1 << 7))));
```

## What differed

```
the more restrictive query is estimated at 1000000000000 rows, the query at 400000000000
```
