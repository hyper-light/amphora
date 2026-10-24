# diff Wrong: Amphora's answer isn't SQLite's

seed 11, state 222

## Statements

```sql
CREATE TABLE t0(c0 INTEGER COLLATE RTRIM, c1 DOUBLE, c2 DECIMAL(10,2), c3 DECIMAL(10,2) UNIQUE);
INSERT OR IGNORE INTO t0 VALUES (x'ff', 9007199254740993, 'ABC', 255), (x'', 7, 0, '1');
```

## Queries

```sql
SELECT (3 / (a1.c1 || a2.c1)) FROM t0 AS a0, t0 AS a1, t0 AS a2;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 8 rows
    3.3306690738754696e-16
    3.3306690738754696e-16
    3.3306690738754696e-16
    3.3306690738754696e-16
    0.4231268741916008
    0.4231268741916008
    ...
  Amphora:  8 rows
    3.33066907387547e-160
    3.33066907387547e-160
    0.4231311706629055
    0.4231311706629055
    3.3306690738754698e-158
    3.3306690738754698e-158
    ...
```
