# cert Estimate: the more restrictive query is estimated at 1000000 rows, the query at 213533.5283236613

seed 14, state 11978

## Statements

```sql
CREATE TABLE t0(c0 BOOLEAN COLLATE RTRIM, c1 BOOLEAN PRIMARY KEY);
INSERT INTO t0 VALUES (-2.5, NULL), ('1', '0');
CREATE TABLE t1(c0 , c1 INT);
ANALYZE;
```

## Queries

```sql
SELECT NULL FROM t1 AS a0 LEFT JOIN t0 AS a1 ON NULL WHERE (a1.c1 IS -1);
SELECT NULL FROM t1 AS a0 JOIN t0 AS a1 ON NULL WHERE (a1.c1 IS -1);
```

## What differed

```
the more restrictive query is estimated at 1000000 rows, the query at 213533.5283236613
```
