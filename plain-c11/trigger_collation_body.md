# diff Wrong: trigger_collation_body

plain

## Statements

```sql
CREATE TABLE u(id INTEGER PRIMARY KEY, name TEXT COLLATE NOCASE);
CREATE TABLE lg(a, b);
CREATE TRIGGER g AFTER INSERT ON u BEGIN INSERT INTO lg VALUES (new.name, new.name = 'admin'); END;
INSERT INTO u VALUES (1, 'ADMIN');
```

## Queries

```sql
SELECT * FROM lg;
```

## What differed

```
```
