# cert Estimate: the more restrictive query is estimated at 1000000000000000000 rows, the query at 100000000000000000

seed 12, state 1105

## Statements

```sql
CREATE TABLE t0(c0 VARCHAR(10) PRIMARY KEY NOT NULL);
```

## Queries

```sql
SELECT 255 FROM t0 AS a0, t0 AS a1, t0 AS a2 WHERE (date((a2.c0 = a2.c0), (a0.c0 / 9223372036854775807), CAST(a1.c0 AS INTEGER)) IS NOT DISTINCT FROM (jsonb_set('_', a1.c0) COLLATE NOCASE));
SELECT 255 FROM t0 AS a0, t0 AS a1, t0 AS a2 WHERE ((date((a2.c0 = a2.c0), (a0.c0 / 9223372036854775807), CAST(a1.c0 AS INTEGER)) IS NOT DISTINCT FROM (jsonb_set('_', a1.c0) COLLATE NOCASE)) AND (CAST(a0.c0 AS INTEGER) IN (2, (CASE WHEN (100 >= a2.c0) THEN 3.0 END))));
```

## What differed

```
the more restrictive query is estimated at 1000000000000000000 rows, the query at 100000000000000000
```
