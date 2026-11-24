# pqs Error: Amphora errors where SQLite finds the pivot: Parse error: insufficient registers allocated for expression vector write (start=9, count=3, next_free=10)

seed 8004, state 30

## Statements

```sql
CREATE TABLE t0(c0 BOOLEAN);
INSERT INTO t0 VALUES ('true');
INSERT INTO t0 VALUES (x'');
INSERT OR IGNORE INTO t0 VALUES ('1.5'), (9007199254740993), ('a');
INSERT OR REPLACE INTO t0 VALUES (1);
```

## Queries

```sql
SELECT ((('{"a":null}', x'41', json_extract('[1,2,3]', '$')) > (NULL, ' a', a0.c0))) IS TRUE, ((('{"a":null}', x'41', json_extract('[1,2,3]', '$')) > (NULL, ' a', a0.c0))) IS FALSE FROM t0 AS a0 WHERE a0.rowid = 6;
SELECT a0.rowid FROM t0 AS a0 WHERE ((('{"a":null}', x'41', json_extract('[1,2,3]', '$')) > (NULL, ' a', a0.c0)) ISNULL);
```

## What differed

```
Amphora errors where SQLite finds the pivot: Parse error: insufficient registers allocated for expression vector write (start=9, count=3, next_free=10)
```
