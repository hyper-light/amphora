# diff Wrong: insert_or_ignore_not_null

plain

## Statements

```sql
CREATE TABLE t(k BLOB PRIMARY KEY NOT NULL);
INSERT OR IGNORE INTO t VALUES (NULL), (x'41'), (NULL);
```

## Queries

```sql
SELECT * FROM t;
```

## What differed

```
```
