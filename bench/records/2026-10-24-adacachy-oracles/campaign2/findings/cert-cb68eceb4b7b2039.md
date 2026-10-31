# cert Estimate: the more restrictive query is estimated at 900000000000 rows, the query at 360000000000

seed 12, state 717

## Statements

```sql
CREATE TABLE t0(c0 TEXT, c1 BOOLEAN COLLATE NOCASE PRIMARY KEY);
```

## Queries

```sql
SELECT NULL FROM t0 AS a0 LEFT JOIN t0 AS a1 ON (a1.c1 <> '_') WHERE ((CASE WHEN x'41' THEN a1.c0 ELSE a0.c1 END) BETWEEN a0.c0 AND 1.5);
SELECT NULL FROM t0 AS a0 JOIN t0 AS a1 ON (a1.c1 <> '_') WHERE ((CASE WHEN x'41' THEN a1.c0 ELSE a0.c1 END) BETWEEN a0.c0 AND 1.5);
```

## What differed

```
the more restrictive query is estimated at 900000000000 rows, the query at 360000000000
```
