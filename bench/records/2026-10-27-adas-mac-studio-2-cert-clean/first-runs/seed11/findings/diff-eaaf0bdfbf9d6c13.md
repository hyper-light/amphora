# diff Wrong: Amphora's answer isn't SQLite's

seed 11, state 77132

## Statements

```sql
CREATE TABLE t0(c0 REAL);
INSERT OR IGNORE INTO t0 VALUES (NULL);
CREATE TABLE t2(c0 TEXT COLLATE NOCASE);
CREATE INDEX i0 ON t2(c0 COLLATE NOCASE);
```

## Queries

```sql
SELECT NULL FROM t0 AS a0 FULL JOIN t2 AS a1 ON (a1.c0 LIKE 'A%');
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 0 rows
  Amphora:  1 rows
    NULL
```
