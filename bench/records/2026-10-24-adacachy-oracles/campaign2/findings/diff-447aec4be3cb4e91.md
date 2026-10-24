# diff Wrong: Amphora's answer isn't SQLite's

seed 12, state 3140

## Statements

```sql
CREATE TABLE t0(c0 INT COLLATE NOCASE UNIQUE, c1 BOOLEAN, c2 DECIMAL(10,2) PRIMARY KEY);
INSERT OR IGNORE INTO t0 VALUES (0.0, '10', '');
CREATE TABLE t1(c0 BLOB PRIMARY KEY);
INSERT OR IGNORE INTO t1 VALUES (NULL), (NULL);
```

## Queries

```sql
SELECT count(*) FROM t0 AS a0, t0 AS a1 LEFT JOIN t1 AS a2 ON ((0 NOT GLOB '%') BETWEEN CAST(0.0 AS INTEGER) AND (CASE a1.c0 WHEN 255 THEN a2.c0 ELSE a1.c1 END));
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    2
  Amphora:  1 rows
    1
```
