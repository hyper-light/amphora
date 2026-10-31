# diff Wrong: Amphora's answer isn't SQLite's

seed 11, state 170

## Statements

```sql
CREATE TABLE t1(c0 DECIMAL(10,2) COLLATE RTRIM UNIQUE, c1 REAL COLLATE BINARY);
INSERT OR IGNORE INTO t1 VALUES (7, NULL), (9007199254740993, NULL);
```

## Queries

```sql
SELECT (a1.c1 % '1'), (a1.c0 / 10), count(*) FROM t1 AS a0, t1 AS a1 GROUP BY (a1.c1 % '1'), (a1.c0 / 10) HAVING (SELECT min(s8.c0) FROM t1 AS s8 WHERE ((a0.c0 NOT IN (0.0, s8.c0)) OR (a1.c1 LIKE 'a%')));
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 2 rows
    NULL, 0, 2
    NULL, 900719925474099, 2
  Amphora:  0 rows
```
