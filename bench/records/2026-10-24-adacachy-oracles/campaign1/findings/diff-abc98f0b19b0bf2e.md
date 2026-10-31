# diff Wrong: Amphora's answer isn't SQLite's

seed 11, state 577

## Statements

```sql
CREATE TABLE t0(c0 , c1 REAL, c2 NUMERIC PRIMARY KEY, c3 BOOLEAN UNIQUE);
INSERT INTO t0 VALUES ('a ', 9007199254740993, 0.0, 'true');
CREATE TABLE t1(c0 BLOB, c1 BLOB, c2 TEXT, c3 BOOLEAN);
INSERT INTO t1 VALUES (-9223372036854775808, -2.5, 'a', 10);
CREATE TABLE t2(c0 REAL);
INSERT OR IGNORE INTO t2 VALUES (x'0102');
```

## Queries

```sql
SELECT '_', json_extract('{"a":null}', '$[0]'), count(*) FROM t0 AS a0, t2 AS a1, t1 AS a2 GROUP BY '_', json_extract('{"a":null}', '$[0]') HAVING (EXISTS (SELECT 1 FROM t1 AS s578 WHERE (a2.c1 <> a0.c3)));
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    '_', NULL, 1
  Amphora:  0 rows
```
