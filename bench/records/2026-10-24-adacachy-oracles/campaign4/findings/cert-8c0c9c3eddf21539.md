# cert Estimate: the more restrictive query is estimated at 64000000000.000015 rows, the query at 2000000000

seed 14, state 6633

## Statements

```sql
CREATE TABLE t0(c0  COLLATE RTRIM UNIQUE, c1 INT UNIQUE);
CREATE TABLE t2(c0 DOUBLE, c1 NUMERIC, c2 BOOLEAN COLLATE BINARY PRIMARY KEY NOT NULL, c3 DOUBLE);
```

## Queries

```sql
SELECT ((x'41' = 9007199254740993) AND (a1.c1 NOT BETWEEN 'x' AND a0.c3)) FROM t2 AS a0, t0 AS a1 WHERE (a0.c0 BETWEEN (a0.c3 >= a0.c1) AND ('1.5' IS DISTINCT FROM a1.c1));
SELECT ((x'41' = 9007199254740993) AND (a1.c1 NOT BETWEEN 'x' AND a0.c3)) FROM t2 AS a0, t0 AS a1 WHERE ((a0.c0 BETWEEN (a0.c3 >= a0.c1) AND ('1.5' IS DISTINCT FROM a1.c1)) AND ((1 NOTNULL) OR (a1.c1 BETWEEN -1 AND -2.5)));
```

## What differed

```
the more restrictive query is estimated at 64000000000.000015 rows, the query at 2000000000
```
