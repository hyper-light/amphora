# cert Estimate: the more restrictive query is estimated at 32000000000 rows, the query at 5000000000

seed 14, state 10363

## Statements

```sql
CREATE TABLE t0(c0 NUMERIC COLLATE RTRIM, c1  PRIMARY KEY, c2 REAL COLLATE NOCASE);
CREATE TABLE t1(c0 , c1 BLOB COLLATE BINARY NOT NULL, c2  UNIQUE);
```

## Queries

```sql
SELECT json_extract('{"a":1,"b":[2,3]}', '$[0]') FROM t1 AS a0, t0 AS a1 WHERE (a0.c2 BETWEEN (a1.c2 ISNULL) AND rtrim('1', ('1e3' COLLATE NOCASE)));
SELECT json_extract('{"a":1,"b":[2,3]}', '$[0]') FROM t1 AS a0, t0 AS a1 WHERE ((a0.c2 BETWEEN (a1.c2 ISNULL) AND rtrim('1', ('1e3' COLLATE NOCASE))) AND ((a1.c2 + a1.c1) NOT GLOB '[a-z]*'));
```

## What differed

```
the more restrictive query is estimated at 32000000000 rows, the query at 5000000000
```
