# pqs Wrong: the pivot (rowid 12), whose predicate SQLite rectified to TRUE, is missing

seed 11, state 804

## Statements

```sql
CREATE TABLE t0(c0 DECIMAL(10,2), c1 NUMERIC, c2 VARCHAR(10), c3 REAL PRIMARY KEY);
INSERT OR IGNORE INTO t0 VALUES ('0', -9223372036854775808, x'00', NULL), (NULL, '1', 2, 3), (0.5, 10, 'abc', 0.1);
INSERT OR IGNORE INTO t0 VALUES (0.0, x'ff', x'ff', 255), (2.0, NULL, 9007199254740993, 10);
INSERT INTO t0 VALUES (7, -9223372036854775808, 1e308, '%');
INSERT INTO t0 VALUES ('A', 'a ', ' a', 'true'), (NULL, x'0102', 1, ' a'), (x'00', 1000000000000000.0, -9223372036854775808, x'ff');
INSERT INTO t0 VALUES (-9223372036854775808, NULL, 255, -2.5), ('-1', 10, 3, -9223372036854775808), ('a ', ' a', 10, '');
```

## Queries

```sql
SELECT (format(a0.c3)) IS TRUE, (format(a0.c3)) IS FALSE FROM t0 AS a0 WHERE a0.rowid = 12;
SELECT a0.rowid FROM t0 AS a0 WHERE (NOT format(a0.c3));
```

## What differed

```
the pivot (rowid 12), whose predicate SQLite rectified to TRUE, is missing
  Amphora: 4 rows
    8
    6
    7
    9
```
