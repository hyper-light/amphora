# tlp Wrong: the query's rows aren't the UNION of its partitions'

seed 20261024, state 785

## Statements

```sql
CREATE TABLE t0(c0 TEXT, c1 VARCHAR(10) COLLATE BINARY UNIQUE);
INSERT INTO t0 VALUES ('0x10', 2);
CREATE TABLE t1(c0 VARCHAR(10), c1 VARCHAR(10) PRIMARY KEY, c2 VARCHAR(10) COLLATE NOCASE, c3 DECIMAL(10,2) UNIQUE);
```

## Queries

```sql
SELECT (x'ff' NOTNULL) FROM t1 AS a0, t1 AS a1 RIGHT JOIN t0 AS a2 ON (a2.c1 GLOB '_');
SELECT (x'ff' NOTNULL) FROM t1 AS a0, t1 AS a1 RIGHT JOIN t0 AS a2 ON (a2.c1 GLOB '_') WHERE (((a2.c0 COLLATE BINARY) - (- a1.c0)) BETWEEN ('_' BETWEEN a0.c0 AND a2.c1) AND log10('10')) UNION SELECT (x'ff' NOTNULL) FROM t1 AS a0, t1 AS a1 RIGHT JOIN t0 AS a2 ON (a2.c1 GLOB '_') WHERE (NOT (((a2.c0 COLLATE BINARY) - (- a1.c0)) BETWEEN ('_' BETWEEN a0.c0 AND a2.c1) AND log10('10'))) UNION SELECT (x'ff' NOTNULL) FROM t1 AS a0, t1 AS a1 RIGHT JOIN t0 AS a2 ON (a2.c1 GLOB '_') WHERE ((((a2.c0 COLLATE BINARY) - (- a1.c0)) BETWEEN ('_' BETWEEN a0.c0 AND a2.c1) AND log10('10')) ISNULL);
```

## What differed

```
the query's rows aren't the UNION of its partitions'
  expected: 1 rows
    1
  Amphora:  0 rows
```
