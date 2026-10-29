# tlp Wrong: the query's rows aren't its three partitions' (p, NOT p, p IS NULL)

seed 13, state 6519

## Statements

```sql
CREATE TABLE t0(c0  COLLATE RTRIM, c1 TEXT COLLATE NOCASE PRIMARY KEY, c2 DOUBLE COLLATE BINARY, c3 INTEGER COLLATE BINARY);
INSERT OR IGNORE INTO t0 VALUES ('0x10', x'00', 9007199254740993, x'00'), (NULL, '1.5', 'a', 7);
```

## Queries

```sql
SELECT sin((a0.c0 / '%')) FROM t0 AS a0, t0 AS a1;
SELECT sin((a0.c0 / '%')) FROM t0 AS a0, t0 AS a1 WHERE ((julianday(0.1) || a0.c1) < a1.c3);
SELECT sin((a0.c0 / '%')) FROM t0 AS a0, t0 AS a1 WHERE (NOT ((julianday(0.1) || a0.c1) < a1.c3));
SELECT sin((a0.c0 / '%')) FROM t0 AS a0, t0 AS a1 WHERE (((julianday(0.1) || a0.c1) < a1.c3) ISNULL);
```

## What differed

```
the query's rows aren't its three partitions' (p, NOT p, p IS NULL)
  expected: 4 rows
    NULL
    NULL
    NULL
    NULL
  Amphora:  3 rows
    NULL
    NULL
    NULL
```
