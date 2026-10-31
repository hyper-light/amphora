# diff Wrong: Amphora's answer isn't SQLite's

seed 20261026, state 39445

## Statements

```sql
CREATE TABLE t1(c0 VARCHAR(10) NOT NULL);
INSERT OR IGNORE INTO t1 VALUES (9223372036854775807);
INSERT OR IGNORE INTO t1 VALUES ('1.5');
INSERT OR IGNORE INTO t1 VALUES (-9223372036854775808);
```

## Queries

```sql
SELECT (SELECT sum(s753.c0) FROM t1 AS s753) FROM t1 AS a0;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 3 rows
    0.5
    0.5
    0.5
  Amphora:  3 rows
    1.5
    1.5
    1.5
```
