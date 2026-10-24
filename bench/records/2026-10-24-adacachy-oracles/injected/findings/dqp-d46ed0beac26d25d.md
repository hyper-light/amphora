# dqp Wrong: another plan gives other rows

seed 20261024, state 265

## Statements

```sql
CREATE TABLE t0(c0 INTEGER PRIMARY KEY, c1 INT UNIQUE, c2 TEXT COLLATE BINARY, c3 NUMERIC COLLATE RTRIM);
INSERT OR IGNORE INTO t0 VALUES (3, 10, '0x10', '0x10');
```

## Queries

```sql
SELECT -9223372036854775808 FROM t0 AS a0 RIGHT JOIN t0 AS a1 ON NULL WHERE (a0.c3 ISNULL);
SELECT -9223372036854775808 FROM t0 AS a0 RIGHT JOIN t0 AS a1 ON NULL WHERE unlikely((a0.c3 ISNULL));
```

## What differed

```
another plan gives other rows
  expected: 0 rows
  Amphora:  1 rows
    -9223372036854775808
```
