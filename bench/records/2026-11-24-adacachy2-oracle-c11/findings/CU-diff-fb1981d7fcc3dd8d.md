# diff Wrong: Amphora's answer isn't SQLite's

seed 8005, state 23387

## Statements

```sql
CREATE TABLE t0(c0 VARCHAR(10) NOT NULL);
INSERT INTO t0 VALUES (-1);
```

## Queries

```sql
WITH RECURSIVE w0(c0, c1) AS (SELECT (~ 0), 0 FROM t0 AS b0 UNION ALL SELECT NULL, 1 FROM w0 AS w0 JOIN t0 AS b1 ON (b1.c0 = w0.c0)) SELECT NULL FROM w0 AS a0;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 2 rows
    NULL
    NULL
  Amphora:  1 rows
    NULL
```
