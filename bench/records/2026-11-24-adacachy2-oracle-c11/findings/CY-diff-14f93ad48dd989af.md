# diff Wrong: Amphora's answer isn't SQLite's

seed 8002, state 14327

## Statements

```sql
CREATE TABLE t0(c0 BOOLEAN PRIMARY KEY, c1 INTEGER COLLATE BINARY, c2 TEXT);
INSERT OR REPLACE INTO t0 VALUES (9223372036854775807, '0', ' ');
INSERT OR IGNORE INTO t0 VALUES (x'', 'A', '');
```

## Queries

```sql
SELECT a0.c2 FROM t0 AS a0 EXCEPT SELECT (NULL COLLATE RTRIM) FROM t0 AS a0;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 2 rows
    ''
    ' '
  Amphora:  1 rows
    ''
```
