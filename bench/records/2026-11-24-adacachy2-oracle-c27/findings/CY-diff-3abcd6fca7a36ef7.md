# diff Wrong: Amphora's answer isn't SQLite's

seed 27006, state 13036

## Statements

```sql
CREATE TABLE t0(c0 INTEGER COLLATE RTRIM NOT NULL);
INSERT OR IGNORE INTO t0 VALUES (' ');
```

## Queries

```sql
SELECT ('' COLLATE NOCASE) FROM t0 AS a0 UNION SELECT a0.c0 FROM t0 AS a0 UNION SELECT 255 FROM t0 AS a0;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 3 rows
    255
    ''
    ' '
  Amphora:  2 rows
    255
    ' '
```
