# diff Wrong: Amphora's answer isn't SQLite's

seed 20261024, state 694

## Statements

```sql
CREATE TABLE t0(c0 BLOB, c1 REAL, c2 BOOLEAN COLLATE BINARY);
INSERT OR IGNORE INTO t0 VALUES ('-1', 0.5, 10);
CREATE TABLE t1(c0 BLOB PRIMARY KEY NOT NULL);
INSERT OR IGNORE INTO t1 VALUES (x'ff');
```

## Queries

```sql
SELECT jsonb_array(CAST(a1.c0 AS TEXT)) FROM t0 AS a0, t1 AS a1;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    x'2b17ff'
  Amphora:  1 rows
    x'4b37efbfbd'
```
