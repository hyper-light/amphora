# diff Wrong: upsert_strict_pk

plain

## Statements

```sql
CREATE TABLE item(id INTEGER PRIMARY KEY, qty INT, last INT) STRICT;
INSERT INTO item VALUES (10, 1, 0);
INSERT INTO item VALUES (10, 5, 0) ON CONFLICT(id) DO UPDATE SET qty = item.qty + excluded.qty, last = excluded.id;
```

## Queries

```sql
SELECT * FROM item;
```

## What differed

```
```
