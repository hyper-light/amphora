# diff Wrong: strict_without_rowid_order

plain

## Statements

```sql
CREATE TABLE a(k INT PRIMARY KEY) WITHOUT ROWID, STRICT;
CREATE TABLE b(k INT PRIMARY KEY) STRICT, WITHOUT ROWID;
INSERT INTO a VALUES (1);
INSERT INTO b VALUES (1);
```

## Queries

```sql
SELECT * FROM a;
SELECT * FROM b;
```

## What differed

```
```
