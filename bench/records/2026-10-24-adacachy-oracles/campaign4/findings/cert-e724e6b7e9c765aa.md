# cert Estimate: the more restrictive query is estimated at 3.6 rows, the query at 1.0979233446124312

seed 14, state 15709

## Statements

```sql
CREATE TABLE t0(c0 INTEGER COLLATE NOCASE PRIMARY KEY, c1 DECIMAL(10,2));
INSERT OR IGNORE INTO t0 VALUES ('1e3', 1);
INSERT INTO t0 VALUES ('0', 'x');
INSERT OR IGNORE INTO t0 VALUES (-1, x'0102');
ANALYZE;
```

## Queries

```sql
SELECT a2.c0 FROM t0 AS a0 FULL JOIN t0 AS a1 ON (NOT (a0.c1 IS NOT 'x')), t0 AS a2 WHERE ((json_quote(a0.c0) == a1.c0) AND (((NOT (a2.c0 != a1.c0)) AND ('abc' <> 10)) AND (a2.c1 > a1.c0)));
SELECT a2.c0 FROM t0 AS a0 JOIN t0 AS a1 ON (NOT (a0.c1 IS NOT 'x')), t0 AS a2 WHERE ((json_quote(a0.c0) == a1.c0) AND (((NOT (a2.c0 != a1.c0)) AND ('abc' <> 10)) AND (a2.c1 > a1.c0)));
```

## What differed

```
the more restrictive query is estimated at 3.6 rows, the query at 1.0979233446124312
```
