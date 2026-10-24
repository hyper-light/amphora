# dqp Wrong: another plan gives other rows

seed 14, state 14272

## Statements

```sql
CREATE TABLE t1(c0 VARCHAR(10), c1 , c2 REAL COLLATE RTRIM);
INSERT OR IGNORE INTO t1 VALUES (-9223372036854775808, 1.5, 9223372036854775807);
INSERT INTO t1 VALUES ('é', 10, 'A');
INSERT INTO t1 VALUES ('é', '1', -100);
CREATE INDEX i2 ON t1(c2 COLLATE NOCASE, (c1 + 1) COLLATE NOCASE DESC);
```

## Queries

```sql
SELECT ('%' * a0.c2), NULL, count(*) FROM t1 AS a0 GROUP BY ('%' * a0.c2), a0.c0;
SELECT ('%' * a0.c2), NULL, count(*) FROM t1 AS a0 INDEXED BY i2 GROUP BY ('%' * a0.c2), a0.c0;
```

## What differed

```
another plan gives other rows
  expected: 2 rows
    0.0, NULL, 1
    0, NULL, 2
  Amphora:  2 rows
    0.0, NULL, 1
    -0.0, NULL, 2
```
