# dqp Wrong: another plan gives other rows

seed 12, state 400

## Statements

```sql
CREATE TABLE t0(c0 VARCHAR(10), c1 BOOLEAN PRIMARY KEY, c2 VARCHAR(10), c3 REAL COLLATE RTRIM);
INSERT INTO t0 VALUES (x'41', -1, 'abc', '1.5'), ('1', x'0102', x'00', -9223372036854775808);
INSERT OR IGNORE INTO t0 VALUES ('A', 7, NULL, 100);
CREATE INDEX i0 ON t0(c3 DESC);
```

## Queries

```sql
SELECT a0.c3 FROM t0 AS a0 ORDER BY ' ' DESC, ' ' DESC, x'00' DESC, x'00' DESC LIMIT 2 OFFSET 2;
SELECT a0.c3 FROM t0 AS a0 NOT INDEXED ORDER BY ' ' DESC, ' ' DESC, x'00' DESC, x'00' DESC LIMIT 2 OFFSET 2;
```

## What differed

```
another plan gives other rows
  expected: 1 rows
    -9.223372036854776e18
  Amphora:  1 rows
    100.0
```
