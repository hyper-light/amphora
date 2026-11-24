# diff Wrong: insert_or_ignore_not_null_wr

plain

## Statements

```sql
CREATE TABLE t(k BLOB PRIMARY KEY) WITHOUT ROWID;
INSERT OR IGNORE INTO t VALUES (NULL), (x'41'), (NULL);
```

## Queries

```sql
SELECT * FROM t;
```

## What differed

```
```
