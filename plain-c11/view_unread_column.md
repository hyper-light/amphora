# diff Wrong: view_unread_column

plain

## Statements

```sql
CREATE TABLE doc(id INTEGER PRIMARY KEY, body TEXT);
INSERT INTO doc VALUES (1, '{"a":1}'), (2, 'not json');
CREATE VIEW v AS SELECT id, json_extract(body, '$.a') AS a FROM doc;
```

## Queries

```sql
SELECT count(*) FROM v;
SELECT id FROM v;
SELECT EXISTS (SELECT 1 FROM v);
SELECT id FROM doc WHERE EXISTS (SELECT 1 FROM v WHERE v.id = doc.id);
SELECT id FROM doc WHERE id IN (SELECT id FROM v);
```

## What differed

```
```
