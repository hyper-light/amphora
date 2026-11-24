# diff Wrong: window_in_order_by

plain

## Statements

```sql
CREATE TABLE t(id INTEGER PRIMARY KEY, v INT);
INSERT INTO t VALUES (1, 5), (2, 3);
```

## Queries

```sql
SELECT id FROM t ORDER BY lag(v + 1) OVER (ORDER BY id), typeof(lag(v + 1) OVER (ORDER BY id)) LIMIT 2;
```

## What differed

```
```
