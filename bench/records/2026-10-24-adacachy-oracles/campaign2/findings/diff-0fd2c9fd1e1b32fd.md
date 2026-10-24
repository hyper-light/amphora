# diff Wrong: Amphora's answer isn't SQLite's

seed 12, state 3241

## Statements

```sql
CREATE TABLE t0(c0 VARCHAR(10), c1 TEXT COLLATE RTRIM, c2 NUMERIC COLLATE NOCASE, c3 DECIMAL(10,2) UNIQUE NOT NULL);
CREATE TABLE t2(c0  COLLATE BINARY PRIMARY KEY, c1 TEXT);
INSERT INTO t2 VALUES (x'41', x'00');
```

## Queries

```sql
SELECT unhex(a1.c1, 0) FROM t0 AS a0 RIGHT JOIN t2 AS a1 ON 'a';
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    x''
  Amphora:  1 rows
    NULL
```
