# diff Wrong: Amphora's answer isn't SQLite's

seed 12, state 3194

## Statements

```sql
CREATE TABLE t1(c0 BOOLEAN COLLATE RTRIM, c1 BLOB, c2 VARCHAR(10) PRIMARY KEY);
INSERT INTO t1 VALUES (0.5, 'é', -1);
```

## Queries

```sql
SELECT exp(a2.c1) FROM t1 AS a0 LEFT JOIN t1 AS a1 ON (json_extract('{"a":1,"b":[2,3]}', '$') BETWEEN floor(a0.c0) AND a1.c1), t1 AS a2 WHERE ((a2.c2 == x'') OR ((0.0 <> a1.c2) OR (a0.c2 == a0.c0)));
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    NULL
  Amphora:  0 rows
```
