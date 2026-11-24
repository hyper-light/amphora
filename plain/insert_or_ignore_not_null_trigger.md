# diff Wrong: insert_or_ignore_not_null_trigger

plain

## Statements

```sql
CREATE TABLE t(k BLOB NOT NULL);
CREATE TABLE lg(a);
CREATE TRIGGER g AFTER INSERT ON t BEGIN INSERT INTO lg VALUES (new.k); END;
INSERT OR IGNORE INTO t VALUES (NULL), (x'41'), (NULL);
```

## Queries

```sql
SELECT * FROM t;
SELECT * FROM lg;
```

## What differed

```
```
