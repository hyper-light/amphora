# diff Wrong: compound_collation

plain

## Statements

```sql
```

## Queries

```sql
SELECT 'x' COLLATE NOCASE UNION SELECT 'X';
SELECT NULL COLLATE NOCASE UNION ALL SELECT 'A' UNION SELECT 'a';
SELECT lower('A') COLLATE NOCASE UNION ALL SELECT 'A' UNION SELECT 'a';
```

## What differed

```
```
