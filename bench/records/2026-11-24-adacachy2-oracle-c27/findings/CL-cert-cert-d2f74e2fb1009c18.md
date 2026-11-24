# cert Estimate: the more restrictive query is estimated at 853814968245.4624 rows, the query at 810000000000

seed 27008, state 2778

## Statements

```sql
CREATE TABLE t0(c0 NUMERIC NOT NULL, c1 BLOB);
```

## Queries

```sql
SELECT NULL FROM t0 AS a0 RIGHT JOIN t0 AS a1 ON ('1e3' <> a1.c0) WHERE (a1.c0 != '1e3');
SELECT NULL FROM t0 AS a0 JOIN t0 AS a1 ON ('1e3' <> a1.c0) WHERE (a1.c0 != '1e3');
```

## What differed

```
the more restrictive query is estimated at 853814968245.4624 rows, the query at 810000000000
```
