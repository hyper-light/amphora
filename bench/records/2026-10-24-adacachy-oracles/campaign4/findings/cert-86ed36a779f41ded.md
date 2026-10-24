# cert Estimate: the more restrictive query is estimated at 2 rows, the query at 0.4270670566473226

seed 14, state 5782

## Statements

```sql
CREATE TABLE t0(c0 DOUBLE);
INSERT INTO t0 VALUES (NULL);
INSERT OR IGNORE INTO t0 VALUES ('-1');
CREATE INDEX i0 ON t0(c0 DESC);
ANALYZE;
```

## Queries

```sql
SELECT NULL FROM t0 AS a0 FULL JOIN t0 AS a1 ON NULL WHERE (NULL IS NOT DISTINCT FROM a1.c0);
SELECT NULL FROM t0 AS a0 JOIN t0 AS a1 ON NULL WHERE (NULL IS NOT DISTINCT FROM a1.c0);
```

## What differed

```
the more restrictive query is estimated at 2 rows, the query at 0.4270670566473226
```
