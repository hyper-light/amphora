# order Estimate: one join order is estimated at 0.3535533905932738 rows, the query as written at 0.5

seed 27008, state 3493

## Statements

```sql
CREATE TABLE t0(c0 BLOB COLLATE RTRIM, c1 BLOB) STRICT;
INSERT OR IGNORE INTO t0 VALUES (x'00', NULL);
ANALYZE;
```

## Queries

```sql
SELECT NULL FROM t0 AS a0 JOIN t0 AS a1 ON ((NULL AND a1.c0) OR a1.c1);
SELECT NULL FROM t0 AS a0, t0 AS a1 WHERE ((NULL AND a1.c0) OR a1.c1);
```

## What differed

```
one join order is estimated at 0.3535533905932738 rows, the query as written at 0.5
```
