# diff Wrong: Amphora's answer isn't SQLite's

seed 8001, state 20890

## Statements

```sql
CREATE TABLE t0(c0 TEXT COLLATE BINARY PRIMARY KEY, c1 );
INSERT OR IGNORE INTO t0 VALUES (1e308, 2.0);
```

## Queries

```sql
SELECT (1 || (a0.c1 / 255)) FROM t0 AS a0;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    '10.0078431372549019607'
  Amphora:  1 rows
    '10.00784313725490196'
```
