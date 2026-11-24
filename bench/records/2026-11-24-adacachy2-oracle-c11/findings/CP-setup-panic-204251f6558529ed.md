# diff Panic: Amphora panicked making the database: UPDATE should contain some updated columns

seed 8003, state 11666

## Statements

```sql
CREATE TABLE t0(c0 BOOLEAN PRIMARY KEY NOT NULL) WITHOUT ROWID;
CREATE TABLE lg(c0, c1, c2);
CREATE TRIGGER g0 AFTER UPDATE OF c0 ON t0 BEGIN INSERT INTO lg VALUES ('g0', NULL, round(old.c0)); END;
CREATE INDEX i0 ON t0(c0);
UPDATE t0 SET c0 = 1000000000000000.0 WHERE c0 IS NOT NULL;
```

## Queries

```sql
```

## What differed

```
Amphora panicked making the database: UPDATE should contain some updated columns
```
