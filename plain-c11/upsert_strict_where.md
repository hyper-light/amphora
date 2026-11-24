# diff Wrong: upsert_strict_where

plain

## Statements

```sql
CREATE TABLE item(id INTEGER PRIMARY KEY, qty INT) STRICT;
INSERT INTO item VALUES (10, 1);
INSERT INTO item VALUES (10, 5) ON CONFLICT(id) DO UPDATE SET qty = excluded.qty WHERE excluded.id = 10;
```

## Queries

```sql
SELECT * FROM item;
```

## What differed

```
```
