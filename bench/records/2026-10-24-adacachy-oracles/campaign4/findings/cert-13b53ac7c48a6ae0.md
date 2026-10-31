# cert Estimate: the more restrictive query is estimated at 160000000000 rows, the query at 5000000000

seed 14, state 13393

## Statements

```sql
CREATE TABLE t0(c0 BLOB COLLATE NOCASE PRIMARY KEY, c1 REAL, c2 REAL, c3 NUMERIC UNIQUE);
```

## Queries

```sql
SELECT a1.c2 FROM t0 AS a0 RIGHT JOIN t0 AS a1 ON ((a1.c3 > 'true') AND (a1.c3 < a0.c1));
SELECT a1.c2 FROM t0 AS a0 JOIN t0 AS a1 ON ((a1.c3 > 'true') AND (a1.c3 < a0.c1));
```

## What differed

```
the more restrictive query is estimated at 160000000000 rows, the query at 5000000000
```
