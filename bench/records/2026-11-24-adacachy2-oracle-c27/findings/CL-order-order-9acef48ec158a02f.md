# order Estimate: one join order is estimated at 0.5 rows, the query as written at 1

seed 27009, state 7600

## Statements

```sql
CREATE TABLE t2(c0 NUMERIC);
INSERT OR IGNORE INTO t2 VALUES (NULL);
ANALYZE;
```

## Queries

```sql
SELECT NULL FROM t2 AS a0 JOIN t2 AS a1 ON ((a1.c0 BETWEEN 2 AND a0.c0) OR a1.c0);
SELECT NULL FROM t2 AS a0, t2 AS a1 WHERE ((a1.c0 BETWEEN 2 AND a0.c0) OR a1.c0);
```

## What differed

```
one join order is estimated at 0.5 rows, the query as written at 1
```
