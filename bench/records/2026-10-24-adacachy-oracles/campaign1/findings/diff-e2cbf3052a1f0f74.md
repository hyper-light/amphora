# diff Wrong: Amphora's answer isn't SQLite's

seed 11, state 640

## Statements

```sql
CREATE TABLE t0(c0 NUMERIC NOT NULL, c1 DOUBLE COLLATE BINARY);
INSERT OR IGNORE INTO t0 VALUES (0, 9007199254740993);
```

## Queries

```sql
SELECT (a1.c0 BETWEEN a0.c1 AND 1), unistr(a2.c1), count(*) FROM t0 AS a0, t0 AS a1, t0 AS a2 GROUP BY (a1.c0 BETWEEN a0.c1 AND 1), unistr(a2.c1);
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    0, '9007199254740992.0', 1
  Amphora:  1 rows
    0, '9.00719925474099e+15', 1
```
