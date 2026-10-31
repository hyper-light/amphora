# pqs Error: Amphora errors where SQLite finds the pivot: Parse error: Unexpected character

seed 12, state 1634

## Statements

```sql
CREATE TABLE t0(c0 INTEGER, c1 DECIMAL(10,2), c2 NUMERIC, c3 REAL);
INSERT INTO t0 VALUES (x'41', NULL, '', 7), (2, 3.0, 10, NULL);
```

## Queries

```sql
SELECT ((('1' == '-1') AND jsonb_replace(a0.c0))) IS TRUE, ((('1' == '-1') AND jsonb_replace(a0.c0))) IS FALSE FROM t0 AS a0 WHERE a0.rowid = 2;
SELECT a0.rowid FROM t0 AS a0 WHERE (NOT (('1' == '-1') AND jsonb_replace(a0.c0)));
```

## What differed

```
Amphora errors where SQLite finds the pivot: Parse error: Unexpected character
```
