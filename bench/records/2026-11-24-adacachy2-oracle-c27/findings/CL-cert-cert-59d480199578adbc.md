# cert Estimate: the more restrictive query is estimated at 94868329805.05139 rows, the query at 90000000000

seed 27009, state 1100

## Statements

```sql
CREATE TABLE lg(c0, c1, c2);
```

## Queries

```sql
SELECT x'' FROM lg AS a0 RIGHT JOIN lg AS a1 ON (a1.c1 IS NULL) WHERE (a1.c2 IS DISTINCT FROM 3);
SELECT x'' FROM lg AS a0 JOIN lg AS a1 ON (a1.c1 IS NULL) WHERE (a1.c2 IS DISTINCT FROM 3);
```

## What differed

```
the more restrictive query is estimated at 94868329805.05139 rows, the query at 90000000000
```
