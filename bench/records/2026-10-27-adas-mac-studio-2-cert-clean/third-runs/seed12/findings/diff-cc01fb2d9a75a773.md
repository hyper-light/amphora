# diff Wrong: Amphora's answer isn't SQLite's

seed 12, state 77836

## Statements

```sql
CREATE TABLE t0(c0 INT COLLATE BINARY UNIQUE, c1 INT COLLATE RTRIM, c2 VARCHAR(10) PRIMARY KEY NOT NULL);
INSERT INTO t0 VALUES ('abc', NULL, 'a');
CREATE TABLE t1(c0 REAL UNIQUE, c1  PRIMARY KEY, c2 NUMERIC);
INSERT INTO t1 VALUES (-1e-308, 3.0, 0.5);
CREATE TABLE t2(c0 INT COLLATE NOCASE NOT NULL, c1 BLOB COLLATE BINARY PRIMARY KEY NOT NULL);
CREATE INDEX i1 ON t2(c0 DESC, c0);
```

## Queries

```sql
SELECT ((a0.c0 COLLATE BINARY) & (9007199254740993 COLLATE BINARY)) FROM t0 AS a0, t1 AS a1 LEFT JOIN t2 AS a2 ON ((a2.c0 LIKE 'a%') AND ((a1.c1 IS DISTINCT FROM NULL) AND (0.1 <= a0.c0)));
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 0 rows
  Amphora:  1 rows
    0
```
