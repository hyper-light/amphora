# diff Wrong: Amphora's answer isn't SQLite's

seed 27002, state 9997

## Statements

```sql
CREATE TABLE t0(c0  COLLATE NOCASE, c1 NUMERIC PRIMARY KEY);
INSERT OR IGNORE INTO t0 VALUES (100, 1000000000000000.0);
```

## Queries

```sql
SELECT jsonb_replace(json_patch(100, '0x10')) FROM t0 AS a0;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    x'4430783130'
  Amphora:  1 rows
    x'233136'
```
