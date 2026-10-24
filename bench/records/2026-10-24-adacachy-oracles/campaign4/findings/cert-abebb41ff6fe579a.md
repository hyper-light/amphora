# cert Estimate: the more restrictive query is estimated at 2 rows, the query at 0.4270670566473226

seed 14, state 23567

## Statements

```sql
CREATE TABLE t0(c0 BLOB, c1 TEXT UNIQUE);
INSERT INTO t0 VALUES (x'0102', NULL);
INSERT INTO t0 VALUES (-1, -100);
ANALYZE;
```

## Queries

```sql
SELECT NULL FROM t0 AS a0 RIGHT JOIN t0 AS a1 ON NULL WHERE (a1.c1 IS NULL);
SELECT NULL FROM t0 AS a0 JOIN t0 AS a1 ON NULL WHERE (a1.c1 IS NULL);
```

## What differed

```
the more restrictive query is estimated at 2 rows, the query at 0.4270670566473226
```
