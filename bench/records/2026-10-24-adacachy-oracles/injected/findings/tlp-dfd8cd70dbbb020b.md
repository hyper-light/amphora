# tlp Wrong: the query's rows aren't its three partitions' (p, NOT p, p IS NULL)

seed 20261024, state 637

## Statements

```sql
CREATE TABLE t0(c0 DOUBLE COLLATE BINARY, c1 VARCHAR(10) COLLATE NOCASE UNIQUE, c2 REAL COLLATE NOCASE UNIQUE, c3 REAL);
CREATE TABLE t1(c0 BLOB COLLATE RTRIM, c1 DOUBLE, c2 BLOB UNIQUE, c3 BOOLEAN COLLATE NOCASE PRIMARY KEY);
INSERT OR IGNORE INTO t1 VALUES ('abc', '0x10', NULL, ' ');
```

## Queries

```sql
SELECT NULL FROM t0 AS a0 FULL JOIN t1 AS a1 ON 'A';
SELECT NULL FROM t0 AS a0 FULL JOIN t1 AS a1 ON 'A' WHERE a0.c2;
SELECT NULL FROM t0 AS a0 FULL JOIN t1 AS a1 ON 'A' WHERE (NOT a0.c2);
SELECT NULL FROM t0 AS a0 FULL JOIN t1 AS a1 ON 'A' WHERE (a0.c2 ISNULL);
```

## What differed

```
the query's rows aren't its three partitions' (p, NOT p, p IS NULL)
  expected: 1 rows
    NULL
  Amphora:  0 rows
```
