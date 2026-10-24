# pqs Wrong: the pivot (rowid 2), whose predicate SQLite rectified to TRUE, is missing

seed 12, state 2297

## Statements

```sql
CREATE TABLE t0(c0 BOOLEAN, c1 DOUBLE COLLATE BINARY PRIMARY KEY, c2 BLOB, c3 INTEGER);
INSERT INTO t0 VALUES (' ', -1, ' ', -9223372036854775808), ('0x10', NULL, NULL, -1);
```

## Queries

```sql
SELECT (unhex(x'00', 0)) IS TRUE, (unhex(x'00', 0)) IS FALSE FROM t0 AS a0 WHERE a0.rowid = 2;
SELECT a0.rowid FROM t0 AS a0 WHERE (NOT unhex(x'00', 0));
```

## What differed

```
the pivot (rowid 2), whose predicate SQLite rectified to TRUE, is missing
  Amphora: 0 rows
```
