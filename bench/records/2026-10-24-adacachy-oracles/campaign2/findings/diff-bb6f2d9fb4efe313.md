# diff Wrong: Amphora's answer isn't SQLite's

seed 12, state 1787

## Statements

```sql
CREATE TABLE t0(c0 INT COLLATE RTRIM, c1 TEXT PRIMARY KEY);
INSERT INTO t0 VALUES (x'00', 10);
CREATE TABLE t2(c0 TEXT);
INSERT OR IGNORE INTO t2 VALUES (1);
```

## Queries

```sql
SELECT NULL FROM t0 AS a0 JOIN t2 AS a1 ON (json_extract('{"a":1,"b":[2,3]}', '$.b[1]') NOT BETWEEN NULL AND a0.c1);
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    NULL
  Amphora:  0 rows
```
