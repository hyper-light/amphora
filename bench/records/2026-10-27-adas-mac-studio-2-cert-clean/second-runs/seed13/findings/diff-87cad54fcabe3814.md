# diff Wrong: Amphora's answer isn't SQLite's

seed 13, state 61318

## Statements

```sql
CREATE TABLE t1(c0 TEXT, c1  PRIMARY KEY NOT NULL, c2 VARCHAR(10) COLLATE BINARY, c3 BLOB UNIQUE);
INSERT OR IGNORE INTO t1 VALUES (x'0102', x'00', -9223372036854775808, 255);
CREATE TABLE t2(c0 BLOB COLLATE NOCASE PRIMARY KEY NOT NULL);
```

## Queries

```sql
SELECT 0.0 FROM t1 AS a0 LEFT JOIN t2 AS a1 ON (a1.c0 LIKE 'a%');
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 0 rows
  Amphora:  1 rows
    0.0
```
