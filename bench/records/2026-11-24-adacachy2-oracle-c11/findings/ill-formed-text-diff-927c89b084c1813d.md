# diff Wrong: Amphora's answer isn't SQLite's

seed 8003, state 603

## Statements

```sql
CREATE TABLE t1(c0 VARCHAR(10) COLLATE NOCASE PRIMARY KEY);
INSERT OR REPLACE INTO t1 VALUES (-2.5);
```

## Queries

```sql
WITH w1(c0, c1) AS MATERIALIZED (SELECT NULL, lower(jsonb(9223372036854775807)) FROM t1 AS b58x0) SELECT CAST(a0.c1 AS BLOB) FROM w1 AS a0;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    x'c31339323233333732303336383534373735383037'
  Amphora:  1 rows
    x'efbfbd1339323233333732303336383534373735383037'
```
