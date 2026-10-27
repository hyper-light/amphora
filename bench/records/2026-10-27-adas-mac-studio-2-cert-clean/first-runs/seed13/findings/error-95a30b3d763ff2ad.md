# pqs Error: Amphora errors where SQLite finds the pivot: Parse error: Unexpected end of input

seed 13, state 14620

## Statements

```sql
CREATE TABLE t1(c0  COLLATE NOCASE, c1 NUMERIC, c2 DOUBLE COLLATE RTRIM);
INSERT INTO t1 VALUES (1000000000000000.0, 1000000000000000.0, 7), (0, x'c3bf', x'');
```

## Queries

```sql
SELECT ((0 NOT BETWEEN (NULL NOT IN (SELECT s119.c1 FROM t1 AS s119 WHERE NULL)) AND jsonb_insert(a0.c2))) IS TRUE, ((0 NOT BETWEEN (NULL NOT IN (SELECT s119.c1 FROM t1 AS s119 WHERE NULL)) AND jsonb_insert(a0.c2))) IS FALSE FROM t1 AS a0 WHERE a0.rowid = 1;
SELECT a0.rowid FROM t1 AS a0 WHERE (0 NOT BETWEEN (NULL NOT IN (SELECT s119.c1 FROM t1 AS s119 WHERE NULL)) AND jsonb_insert(a0.c2));
```

## What differed

```
Amphora errors where SQLite finds the pivot: Parse error: Unexpected end of input
```
