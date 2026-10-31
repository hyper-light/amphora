# diff Wrong: Amphora's answer isn't SQLite's

seed 13, state 2691

## Statements

```sql
CREATE TABLE t0(c0 BOOLEAN COLLATE BINARY PRIMARY KEY, c1 VARCHAR(10) NOT NULL, c2 NUMERIC);
INSERT INTO t0 VALUES ('1', -100, 100);
```

## Queries

```sql
SELECT trim((a1.c0 ISNULL), jsonb_patch(1e308, a1.c1)) FROM t0 AS a0, t0 AS a1;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    '0'
  Amphora:  1 rows
    ''
```
