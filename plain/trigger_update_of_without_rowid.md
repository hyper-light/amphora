# diff Wrong: trigger_update_of_without_rowid

plain

## Statements

```sql
CREATE TABLE t(k TEXT PRIMARY KEY, v TEXT) WITHOUT ROWID;
CREATE TABLE lg(a);
CREATE TRIGGER g AFTER UPDATE OF k ON t BEGIN INSERT INTO lg VALUES (old.k); END;
INSERT INTO t VALUES ('a', 'x');
UPDATE t SET v = 'y' WHERE k = 'a';
UPDATE t SET k = 'b' WHERE v IS NOT NULL;
```

## Queries

```sql
SELECT * FROM t;
SELECT * FROM lg;
```

## What differed

```
```
