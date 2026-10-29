# diff Wrong: Amphora's answer isn't SQLite's

seed 20261028, state 17915

## Statements

```sql
CREATE TABLE t2(c0 BOOLEAN UNIQUE NOT NULL, c1 TEXT);
INSERT OR IGNORE INTO t2 VALUES (x'41', 1e308);
```

## Queries

```sql
SELECT unicode(jsonb_set(a0.c1)) FROM t2 AS a0;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    133
  Amphora:  1 rows
    65533
```
