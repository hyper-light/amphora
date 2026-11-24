# diff Wrong: Amphora's answer isn't SQLite's

seed 27010, state 14353

## Statements

```sql
CREATE TABLE t0(c0 NUMERIC);
CREATE TABLE t1(c0 REAL PRIMARY KEY, c1 INTEGER, c2 NUMERIC UNIQUE, c3  COLLATE BINARY);
INSERT INTO t0 VALUES ('0x10');
INSERT INTO t1 VALUES (NULL, 0.0, 1.5, 9007199254740993);
```

## Queries

```sql
SELECT NULL, NULL, NULL FROM t0 AS a0 GROUP BY a0.c0 HAVING (NULL NOT IN (SELECT s410.c1 FROM t1 AS s410 WHERE (SELECT min(s840.c3) FROM t1 AS s840 WHERE (a0.c0 IS NOT s410.c0))));
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 0 rows
  Amphora:  1 rows
    NULL, NULL, NULL
```
