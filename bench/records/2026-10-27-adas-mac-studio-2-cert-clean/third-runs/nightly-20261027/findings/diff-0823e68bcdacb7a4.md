# diff Wrong: Amphora's answer isn't SQLite's

seed 20261027, state 37990

## Statements

```sql
CREATE TABLE t0(c0  PRIMARY KEY, c1  COLLATE BINARY, c2 DECIMAL(10,2));
CREATE TABLE t1(c0 BOOLEAN COLLATE NOCASE PRIMARY KEY, c1 INTEGER UNIQUE, c2 TEXT, c3 BOOLEAN COLLATE RTRIM);
INSERT OR IGNORE INTO t1 VALUES (3, -1, 9007199254740993, x'0102');
```

## Queries

```sql
SELECT '' FROM t0 AS a0 RIGHT JOIN t1 AS a1 ON (a1.c0 LIKE 'A%');
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 0 rows
  Amphora:  1 rows
    ''
```
