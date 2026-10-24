# cert Estimate: the more restrictive query is estimated at 1000000 rows, the query at 10000.000000000002

seed 14, state 4478

## Statements

```sql
CREATE TABLE t0(c0 BLOB PRIMARY KEY, c1 TEXT NOT NULL);
CREATE TABLE t1(c0 TEXT PRIMARY KEY, c1  UNIQUE);
INSERT OR IGNORE INTO t1 VALUES (1, -1e-308);
ANALYZE;
```

## Queries

```sql
SELECT json_insert(a2.c1, a0.c0) FROM t0 AS a0, t1 AS a1 JOIN t1 AS a2 ON ((a2.c0 + '0') IS (NULL - a2.c0));
SELECT json_insert(a2.c1, a0.c0) FROM t0 AS a0, t1 AS a1 JOIN t1 AS a2 ON ((a2.c0 + '0') IS (NULL - a2.c0)) WHERE ((EXISTS (SELECT 1 FROM t0 AS s792)) AND (a1.c1 AND 0));
```

## What differed

```
the more restrictive query is estimated at 1000000 rows, the query at 10000.000000000002
```
