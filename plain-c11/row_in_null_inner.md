# diff Wrong: row_in_null_inner

plain

## Statements

```sql
CREATE TABLE o(id INTEGER PRIMARY KEY, region TEXT, code INT);
CREATE TABLE closed(region TEXT, code INT);
INSERT INTO o VALUES (1, 'eu', 7), (2, 'us', 8);
INSERT INTO closed VALUES (NULL, 7), ('eu', NULL);
```

## Queries

```sql
SELECT id, (region, code) IN (SELECT region, code FROM closed) FROM o;
SELECT id FROM o WHERE (region, code) NOT IN (SELECT region, code FROM closed);
```

## What differed

```
```
