# diff Wrong: Amphora's answer isn't SQLite's

seed 11, state 271

## Statements

```sql
CREATE TABLE t0(c0 BOOLEAN COLLATE RTRIM, c1 BLOB COLLATE NOCASE NOT NULL);
INSERT OR IGNORE INTO t0 VALUES (NULL, -1);
```

## Queries

```sql
SELECT (x'41' LIKE 'A%') FROM t0 AS a0;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    1
  Amphora:  1 rows
    0
```
