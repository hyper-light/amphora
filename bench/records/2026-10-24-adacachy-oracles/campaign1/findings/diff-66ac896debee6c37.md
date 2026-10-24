# diff Wrong: Amphora's answer isn't SQLite's

seed 11, state 211

## Statements

```sql
CREATE TABLE t0(c0 NUMERIC, c1 DOUBLE, c2 VARCHAR(10));
INSERT INTO t0 VALUES ('_', 0, 10);
INSERT OR IGNORE INTO t0 VALUES (9007199254740993, NULL, 10);
CREATE TABLE t1(c0 BLOB, c1 TEXT COLLATE BINARY, c2 REAL PRIMARY KEY NOT NULL);
INSERT INTO t1 VALUES (255, 3.0, 1000000000000000.0);
```

## Queries

```sql
SELECT (x'ff' BETWEEN a2.c2 AND a2.c2), count(*) FROM t1 AS a0 LEFT JOIN t0 AS a1 ON ((+ a0.c2) BETWEEN a1.c2 AND (a0.c2 COLLATE RTRIM)), t0 AS a2 GROUP BY (x'ff' BETWEEN a2.c2 AND a2.c2);
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    0, 4
  Amphora:  1 rows
    0, 2
```
