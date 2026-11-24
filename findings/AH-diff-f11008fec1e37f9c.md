# diff Wrong: Amphora's answer isn't SQLite's

seed 27008, state 8815

## Statements

```sql
CREATE TABLE t0(c0 REAL, c1 INT);
INSERT OR REPLACE INTO t0 VALUES (3, 255);
CREATE INDEX i0 ON t0(c1 COLLATE NOCASE DESC);
```

## Queries

```sql
SELECT '{"a":1,"b":[2,3]}' FROM t0 AS a0 FULL JOIN t0 AS a1 ON (a1.c1 LIKE 'A%');
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 0 rows
  Amphora:  2 rows
    '{"a":1,"b":[2,3]}'
    '{"a":1,"b":[2,3]}'
```
