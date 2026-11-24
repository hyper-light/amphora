# diff Wrong: trigger_update_without_rowid

plain

## Statements

```sql
CREATE TABLE t(k TEXT PRIMARY KEY, v TEXT) WITHOUT ROWID;
CREATE TABLE lg(a);
CREATE TRIGGER g AFTER UPDATE ON t BEGIN INSERT INTO lg VALUES (old.k); END;
INSERT INTO t VALUES ('a', 'x');
UPDATE t SET v = 'y' WHERE k = 'a';
```

## Queries

```sql
SELECT * FROM t;
SELECT * FROM lg;
```

## What differed

```
```
