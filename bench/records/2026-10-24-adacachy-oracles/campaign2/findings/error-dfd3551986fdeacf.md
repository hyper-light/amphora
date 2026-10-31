# pqs Error: Amphora errors where SQLite finds the pivot: json_object() requires an even number of arguments

seed 12, state 3666

## Statements

```sql
CREATE TABLE t1(c0  PRIMARY KEY, c1 INTEGER, c2 REAL COLLATE RTRIM, c3 DOUBLE UNIQUE NOT NULL);
INSERT OR IGNORE INTO t1 VALUES (9007199254740993, -9223372036854775808, 10, 9007199254740993), ('_', -1e-308, 9223372036854775807, 7);
```

## Queries

```sql
SELECT ((unistr_quote('10') AND (a0.c1 IN ((0 | a0.c1), json_object(NULL, NULL, 0.5), 10)))) IS TRUE, ((unistr_quote('10') AND (a0.c1 IN ((0 | a0.c1), json_object(NULL, NULL, 0.5), 10)))) IS FALSE FROM t1 AS a0 WHERE a0.rowid = 1;
SELECT a0.rowid FROM t1 AS a0 WHERE (NOT (unistr_quote('10') AND (a0.c1 IN ((0 | a0.c1), json_object(NULL, NULL, 0.5), 10))));
```

## What differed

```
Amphora errors where SQLite finds the pivot: json_object() requires an even number of arguments
```
