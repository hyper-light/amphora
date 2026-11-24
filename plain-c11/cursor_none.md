# diff Wrong: cursor_none

plain

## Statements

```sql
CREATE TABLE w(k TEXT PRIMARY KEY, u INT) WITHOUT ROWID;
CREATE TABLE o(k TEXT, n INT);
INSERT INTO w VALUES ('a', 1);
```

## Queries

```sql
SELECT a1.c0 FROM w AS a0, (SELECT CAST((b.k NOT IN (SELECT s.k FROM o AS s WHERE (c.n NOT IN (3)))) AS INTEGER) AS c0 FROM o AS c FULL JOIN w AS b GROUP BY CAST((b.k NOT IN (SELECT s.k FROM o AS s WHERE (c.n NOT IN (3)))) AS INTEGER)) AS a1;
```

## What differed

```
```
