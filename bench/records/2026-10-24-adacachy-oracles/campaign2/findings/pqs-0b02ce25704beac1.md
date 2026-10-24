# pqs Wrong: the pivot (rowid 2), whose predicate SQLite rectified to TRUE, is missing

seed 12, state 2596

## Statements

```sql
CREATE TABLE t0(c0 NUMERIC);
INSERT OR IGNORE INTO t0 VALUES ('_'), (-9223372036854775808);
```

## Queries

```sql
SELECT (jsonb_array_insert(x'00')) IS TRUE, (jsonb_array_insert(x'00')) IS FALSE FROM t0 AS a0 WHERE a0.rowid = 2;
SELECT a0.rowid FROM t0 AS a0 WHERE (NOT jsonb_array_insert(x'00'));
```

## What differed

```
the pivot (rowid 2), whose predicate SQLite rectified to TRUE, is missing
  Amphora: 0 rows
```
