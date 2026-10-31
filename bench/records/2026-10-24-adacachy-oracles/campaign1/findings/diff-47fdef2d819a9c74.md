# diff Wrong: Amphora's answer isn't SQLite's

seed 11, state 907

## Statements

```sql
CREATE TABLE t0(c0 INTEGER, c1 BLOB, c2 BOOLEAN PRIMARY KEY, c3 BOOLEAN COLLATE RTRIM UNIQUE);
INSERT INTO t0 VALUES ('é', x'ff', 'true', x'ff');
CREATE TABLE t1(c0 DOUBLE PRIMARY KEY, c1 TEXT COLLATE RTRIM, c2 INT COLLATE RTRIM, c3 INT);
INSERT OR IGNORE INTO t1 VALUES (3.0, 1.5, 'abc', NULL);
```

## Queries

```sql
SELECT NULL FROM t0 AS a0 WHERE (EXISTS (SELECT 1 FROM t1 AS s247 WHERE ((x'ff' LIKE '%') > (s247.c3 NOTNULL))));
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    NULL
  Amphora:  0 rows
```
