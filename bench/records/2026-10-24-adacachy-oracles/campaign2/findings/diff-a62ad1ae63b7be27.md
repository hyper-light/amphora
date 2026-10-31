# diff Wrong: Amphora's answer isn't SQLite's

seed 12, state 957

## Statements

```sql
CREATE TABLE t0(c0 DECIMAL(10,2) COLLATE BINARY, c1 BLOB);
INSERT OR IGNORE INTO t0 VALUES ('1.5', 9223372036854775807);
CREATE INDEX i0 ON t0(c1 COLLATE NOCASE DESC);
```

## Queries

```sql
SELECT NULL FROM t0 AS a0 RIGHT JOIN t0 AS a1 ON (a1.c1 LIKE 'A%');
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 0 rows
  Amphora:  1 rows
    NULL
```
