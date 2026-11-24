# diff Wrong: compound_order_collate

plain

## Statements

```sql
CREATE TABLE t(a TEXT);
INSERT INTO t VALUES ('b'), ('A');
```

## Queries

```sql
SELECT a FROM t UNION SELECT 'C' ORDER BY 1 COLLATE NOCASE;
SELECT a FROM t UNION SELECT 'C' ORDER BY (1 COLLATE NOCASE);
```

## What differed

```
```
