# diff Wrong: Amphora's answer isn't SQLite's

seed 27005, state 18209

## Statements

```sql
CREATE TABLE t0(c0 TEXT PRIMARY KEY);
INSERT OR IGNORE INTO t0 VALUES ('ABC');
INSERT INTO t0 VALUES ('abc');
```

## Queries

```sql
SELECT a0.c0, NULL FROM t0 AS a0 GROUP BY a0.c0 UNION SELECT (NULL COLLATE NOCASE), 1000000000000000.0 FROM t0 AS a0;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 3 rows
    NULL, 1000000000000000.0
    'ABC', NULL
    'abc', NULL
  Amphora:  2 rows
    NULL, 1000000000000000.0
    'abc', NULL
```
