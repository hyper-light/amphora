# diff Wrong: row_misused

plain

## Statements

```sql
CREATE TABLE a(x INT, y INT);
CREATE TABLE b(x INT, y INT);
INSERT INTO a VALUES (1, 2), (3, 4);
INSERT INTO b VALUES (1, 2);
```

## Queries

```sql
SELECT count(*) FROM a LEFT JOIN b ON (a.x, a.y) <> (SELECT max(x), count(y) FROM b);
SELECT * FROM a WHERE (a.x, a.y) = (SELECT max(x), max(y) FROM b) OR a.x = 3;
SELECT * FROM a WHERE NOT ((a.x, a.y) < (SELECT max(x), max(y) FROM b));
```

## What differed

```
```
