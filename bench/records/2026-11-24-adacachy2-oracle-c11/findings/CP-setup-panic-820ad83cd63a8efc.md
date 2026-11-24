# diff Panic: Amphora panicked making the database: UPDATE should contain some updated columns

seed 8002, state 1686

## Statements

```sql
CREATE TABLE t0(c0 DOUBLE PRIMARY KEY, c1 INTEGER) WITHOUT ROWID;
CREATE TABLE lg(c0, c1, c2);
CREATE TRIGGER g0 BEFORE UPDATE OF c1 ON t0 BEGIN INSERT INTO lg VALUES ('g0', -100, (new.c1 ISNULL)); END;
INSERT INTO t0 VALUES ('x', -1);
CREATE INDEX i0 ON t0(c0, c1);
CREATE UNIQUE INDEX i1 ON t0(c1);
UPDATE t0 SET c1 = 'true' WHERE c0 IS NOT NULL;
```

## Queries

```sql
```

## What differed

```
Amphora panicked making the database: UPDATE should contain some updated columns
```
