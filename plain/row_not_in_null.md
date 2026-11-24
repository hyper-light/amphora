# diff Wrong: row_not_in_null

plain

## Statements

```sql
CREATE TABLE o(id INTEGER PRIMARY KEY, region TEXT, code INT);
CREATE TABLE closed(region TEXT, code INT);
INSERT INTO o VALUES (1, 'eu', 7), (2, NULL, 8), (3, 'us', 9), (4, NULL, 7);
INSERT INTO closed VALUES ('eu', 7);
```

## Queries

```sql
SELECT id FROM o WHERE (region, code) NOT IN (SELECT region, code FROM closed);
SELECT id, (region, code) IN (SELECT region, code FROM closed) FROM o;
SELECT (NULL, 3) IN (SELECT region, code FROM closed);
```

## What differed

```
```
