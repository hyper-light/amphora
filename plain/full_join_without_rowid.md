# diff Wrong: full_join_without_rowid

plain

## Statements

```sql
CREATE TABLE w(k TEXT PRIMARY KEY, u INT UNIQUE) WITHOUT ROWID;
CREATE TABLE o(k TEXT);
INSERT INTO w VALUES ('a', 1), ('b', 2);
INSERT INTO o VALUES ('a'), ('c');
```

## Queries

```sql
SELECT o.k, w.k FROM o FULL JOIN w ON w.k = o.k;
SELECT o.k, w.k FROM o RIGHT JOIN w ON w.u > 1;
SELECT o.k, w.k FROM o FULL JOIN w;
```

## What differed

```
```
