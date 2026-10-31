# cert Estimate: the more restrictive query is estimated at 64000000000.000015 rows, the query at 2000000000

seed 12, state 2137

## Statements

```sql
CREATE TABLE t0(c0 TEXT PRIMARY KEY, c1 INTEGER UNIQUE);
```

## Queries

```sql
SELECT ((x'41' = 9007199254740993) AND (a1.c1 NOT BETWEEN 'x' AND a0.c1)) FROM t0 AS a0, t0 AS a1 WHERE (a0.c0 BETWEEN (a0.c1 >= a0.c1) AND ('1.5' IS DISTINCT FROM a1.c1));
SELECT ((x'41' = 9007199254740993) AND (a1.c1 NOT BETWEEN 'x' AND a0.c1)) FROM t0 AS a0, t0 AS a1 WHERE ((a0.c0 BETWEEN (a0.c1 >= a0.c1) AND ('1.5' IS DISTINCT FROM a1.c1)) AND ((1 NOTNULL) OR (a1.c1 BETWEEN -1 AND -2.5)));
```

## What differed

```
the more restrictive query is estimated at 64000000000.000015 rows, the query at 2000000000
```
