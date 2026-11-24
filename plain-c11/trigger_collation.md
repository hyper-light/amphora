# diff Wrong: trigger_collation

plain

## Statements

```sql
CREATE TABLE u(id INTEGER PRIMARY KEY, name TEXT COLLATE NOCASE);
CREATE TABLE lg(a);
CREATE TRIGGER g AFTER INSERT ON u WHEN new.name = 'admin' BEGIN INSERT INTO lg VALUES (new.name); END;
INSERT INTO u VALUES (1, 'ADMIN');
INSERT INTO u VALUES (2, 'bob');
```

## Queries

```sql
SELECT * FROM lg;
```

## What differed

```
```
